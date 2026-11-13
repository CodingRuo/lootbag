//! macOS drag-out owned by Lootbag: a local event monitor sees the real mouse events, and a
//! prepared payload armed for the held press starts `beginDraggingSession` from the real
//! `leftMouseDragged` event, as AppKit requires. A press that ends first never starts a drag.

use crate::library::{DragGesture, DragItem, DragStage};
use block2::RcBlock;
use objc2::{
    define_class, msg_send, rc::Retained, runtime::ProtocolObject, AnyThread, DefinedClass,
    MainThreadMarker, MainThreadOnly,
};
use objc2_app_kit::{
    NSDragOperation, NSDraggingContext, NSDraggingItem, NSDraggingSession, NSDraggingSource,
    NSEvent, NSEventMask, NSEventType, NSWorkspace,
};
use objc2_foundation::{
    NSArray, NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString, NSURL,
};
use std::{
    cell::RefCell,
    ptr::NonNull,
    sync::{LazyLock, Mutex, MutexGuard, PoisonError},
};
use tauri::{AppHandle, Emitter};

/// A staged payload, or the German reason it could not be prepared (shown only if dragged).
pub type Prepared = Result<DragStage, String>;

static GESTURE: LazyLock<Mutex<DragGesture<Prepared>>> = LazyLock::new(Mutex::default);

thread_local! {
    // The running session's source; kept alive here until the next drag replaces it.
    static SOURCE: RefCell<Option<Retained<DragSource>>> = const { RefCell::new(None) };
}

fn gesture() -> MutexGuard<'static, DragGesture<Prepared>> {
    GESTURE.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The mouse press still held, if any; a preparation binds to it.
pub fn held() -> Option<u64> {
    gesture().held()
}

/// Arms `press` with its payload, unless that press already ended.
pub fn arm(press: u64, prepared: Prepared) {
    gesture().arm(press, prepared);
}

pub fn disarm() {
    gesture().disarm();
}

#[derive(Clone, serde::Serialize)]
struct DragEnd {
    dropped: bool,
    problems: Vec<String>,
    error: Option<String>,
}

/// Installs the event monitor; call once on the main thread.
pub fn install(app: AppHandle) {
    let mask = NSEventMask::LeftMouseDown
        | NSEventMask::LeftMouseDragged
        | NSEventMask::LeftMouseUp
        | NSEventMask::KeyDown;
    let handler = RcBlock::new(move |event: NonNull<NSEvent>| -> *mut NSEvent {
        // SAFETY: AppKit hands the monitor a valid event for the duration of the call.
        let real = unsafe { event.as_ref() };
        let at = real.locationInWindow();
        match real.r#type() {
            NSEventType::LeftMouseDown => gesture().press((at.x, at.y)),
            // Any key while held (Escape, Cmd+A, arrows) may change the selection: never drag stale.
            NSEventType::LeftMouseUp | NSEventType::KeyDown => disarm(),
            NSEventType::LeftMouseDragged => {
                let prepared = gesture().drag((at.x, at.y));
                if let Some(prepared) = prepared {
                    start(&app, real, prepared);
                }
            }
            _ => {}
        }
        event.as_ptr()
    });
    // SAFETY: called on the main thread; the monitor lives as long as the app.
    let monitor = unsafe { NSEvent::addLocalMonitorForEventsMatchingMask_handler(mask, &handler) };
    std::mem::forget(monitor);
}

fn start(app: &AppHandle, event: &NSEvent, prepared: Prepared) {
    let report = |error: String| {
        let _ = app.emit(
            "drag-out-ended",
            DragEnd {
                dropped: false,
                problems: Vec::new(),
                error: Some(error),
            },
        );
    };
    let stage = match prepared {
        Ok(stage) => stage,
        Err(error) => return report(error),
    };
    let Some(mtm) = MainThreadMarker::new() else {
        return report("Ziehen konnte nicht gestartet werden".into());
    };
    let Some(view) = event.window(mtm).and_then(|window| window.contentView()) else {
        return report("Ziehen konnte nicht gestartet werden: kein Fenster".into());
    };
    // Dragging frames are in the view's own coordinates, not the window's.
    let at = view.convertPoint_fromView(event.locationInWindow(), None);
    let items: Vec<_> = stage
        .items
        .iter()
        .enumerate()
        .map(|(index, item)| dragging_item(item, at, index))
        .collect();
    let source = DragSource::new(app.clone(), stage.problems, mtm);
    let session = view.beginDraggingSessionWithItems_event_source(
        &NSArray::from_retained_slice(&items),
        event,
        ProtocolObject::from_ref(&*source),
    );
    session.setAnimatesToStartingPositionsOnCancelOrFail(true);
    SOURCE.with(|slot| *slot.borrow_mut() = Some(source));
    let _ = app.emit("drag-out-started", ());
}

fn dragging_item(item: &DragItem, at: NSPoint, index: usize) -> Retained<NSDraggingItem> {
    let path = NSString::from_str(&item.path.to_string_lossy());
    let url = NSURL::fileURLWithPath_isDirectory(&path, item.directory);
    let icon = NSWorkspace::sharedWorkspace().iconForFile(&path);
    let size = 64.0;
    icon.setSize(NSSize::new(size, size));
    // A small fan keeps several items visible under the pointer.
    let offset = (index.min(4) * 6) as f64;
    let frame = NSRect::new(
        NSPoint::new(at.x - size / 2.0 + offset, at.y - size / 2.0 - offset),
        NSSize::new(size, size),
    );
    let dragging = NSDraggingItem::initWithPasteboardWriter(
        NSDraggingItem::alloc(),
        ProtocolObject::from_ref(&*url),
    );
    // SAFETY: an NSImage is a valid contents object for a dragging frame.
    unsafe { dragging.setDraggingFrame_contents(frame, Some(&icon)) };
    dragging
}

struct SourceIvars {
    app: AppHandle,
    problems: Vec<String>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "LootbagDragSource"]
    #[ivars = SourceIvars]
    struct DragSource;

    unsafe impl NSObjectProtocol for DragSource {}

    unsafe impl NSDraggingSource for DragSource {
        // Copy is the only offered operation, so a recipient never moves the original.
        #[unsafe(method(draggingSession:sourceOperationMaskForDraggingContext:))]
        fn operation_mask(
            &self,
            _session: &NSDraggingSession,
            _context: NSDraggingContext,
        ) -> NSDragOperation {
            NSDragOperation::Copy
        }

        #[unsafe(method(draggingSession:endedAtPoint:operation:))]
        fn ended(&self, _session: &NSDraggingSession, _at: NSPoint, operation: NSDragOperation) {
            let ivars = self.ivars();
            let _ = ivars.app.emit(
                "drag-out-ended",
                DragEnd {
                    dropped: operation != NSDragOperation::None,
                    problems: ivars.problems.clone(),
                    error: None,
                },
            );
        }
    }
);

impl DragSource {
    fn new(app: AppHandle, problems: Vec<String>, mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(SourceIvars { app, problems });
        // SAFETY: plain NSObject init of a freshly allocated instance.
        unsafe { msg_send![super(this), init] }
    }
}
