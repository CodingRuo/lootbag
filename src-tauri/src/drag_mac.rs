//! macOS drag-out owned by Lootbag: a local event monitor sees the real mouse events, and a
//! payload staged for the held press starts `beginDraggingSession` from a real
//! `leftMouseDragged` event of that press, as AppKit requires. A press that ends first never
//! starts a drag.
//!
//! Press identity: the monitor numbers every mouse-down and, before AppKit dispatches the event
//! to the WKWebView under the pointer, sets `window.__lootbagPress` in that webview. Script
//! evaluation and the mouse event travel to the web process in that order over the same
//! connection, so the page's pointerdown reads its own press number and `prepare_drag` sends it
//! back with the invoking window; the gesture refuses any other press or window.

use super::{discard_stage, drag_ended, drag_published, stage_drag};
use crate::library::{DragGesture, DragItem, DragSource, DragStage, DragStep};
use block2::{Block, RcBlock};
use objc2::{
    define_class, msg_send,
    rc::Retained,
    runtime::{AnyClass, AnyObject, ProtocolObject},
    AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly,
};
use objc2_app_kit::{
    NSDragOperation, NSDraggingContext, NSDraggingItem, NSDraggingSession, NSDraggingSource,
    NSEvent, NSEventMask, NSEventType, NSView, NSWorkspace,
};
use objc2_foundation::{
    NSArray, NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString, NSURL,
};
use std::{
    cell::RefCell,
    path::PathBuf,
    ptr::NonNull,
    sync::{LazyLock, Mutex, MutexGuard, PoisonError},
};
use tauri::{AppHandle, Emitter};

/// A staged payload, or the German reason it could not be prepared (shown only if dragged).
pub type Prepared = Result<DragStage, String>;

static GESTURE: LazyLock<Mutex<DragGesture<Vec<DragSource>, Prepared>>> =
    LazyLock::new(Mutex::default);

thread_local! {
    // The running session's source; kept alive here until the next drag replaces it.
    static SOURCE: RefCell<Option<Retained<Source>>> = const { RefCell::new(None) };
}

fn gesture() -> MutexGuard<'static, DragGesture<Vec<DragSource>, Prepared>> {
    GESTURE.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Binds `sources` to press `press` of `window` (an NSWindow pointer), if that press is held.
pub fn request(window: usize, press: u64, sources: Vec<DragSource>) {
    gesture().request(window, press, sources);
}

pub fn disarm() {
    discard(gesture().disarm());
}

/// Removes the copies of a payload no drag will use.
fn discard(prepared: Option<Prepared>) {
    if let Some(Ok(stage)) = prepared {
        discard_stage(&stage);
    }
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
            NSEventType::LeftMouseDown => {
                let Some(mtm) = MainThreadMarker::new() else {
                    return event.as_ptr();
                };
                let window = real.window(mtm);
                let id = window.as_ref().map_or(0, |w| Retained::as_ptr(w) as usize);
                let (press, dropped) = gesture().press(id, (at.x, at.y));
                discard(dropped);
                if let Some(view) = window.and_then(|window| window.contentView()) {
                    announce(&view, at, press);
                }
            }
            // Any key while held (Escape, Cmd+A, arrows) may change the selection: never drag stale.
            NSEventType::LeftMouseUp | NSEventType::KeyDown => disarm(),
            NSEventType::LeftMouseDragged => {
                let step = gesture().drag((at.x, at.y));
                match step {
                    DragStep::Wait => {}
                    DragStep::Stage(press, sources) => {
                        let app = app.clone();
                        tauri::async_runtime::spawn(async move {
                            let prepared = stage_drag(app, sources).await;
                            let refused = gesture().arm(press, prepared).err();
                            discard(refused);
                        });
                    }
                    DragStep::Start(prepared) => start(&app, real, prepared),
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

/// Tells the webview under the press its number, ahead of the mouse event itself.
fn announce(content: &NSView, at: NSPoint, press: u64) {
    let Some(class) = AnyClass::get(c"WKWebView") else {
        return;
    };
    let mut view = content.hitTest(at);
    while let Some(current) = view {
        if current.isKindOfClass(class) {
            let script = NSString::from_str(&format!("window.__lootbagPress={press}"));
            let done: Option<&Block<dyn Fn(*mut AnyObject, *mut AnyObject)>> = None;
            // SAFETY: `current` is a WKWebView; a nil completion handler is allowed.
            let () = unsafe {
                msg_send![&*current, evaluateJavaScript: &*script, completionHandler: done]
            };
            return;
        }
        // SAFETY: walking up the view tree of a live window on the main thread.
        view = unsafe { current.superview() };
    }
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
    let view = MainThreadMarker::new()
        .and_then(|mtm| event.window(mtm))
        .and_then(|window| window.contentView());
    let (Some(mtm), Some(view)) = (MainThreadMarker::new(), view) else {
        discard_stage(&stage);
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
    drag_published(&stage);
    let source = Source::new(app.clone(), stage.problems, stage.batch, mtm);
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
    batch: Option<PathBuf>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "LootbagDragSource"]
    #[ivars = SourceIvars]
    struct Source;

    unsafe impl NSObjectProtocol for Source {}

    unsafe impl NSDraggingSource for Source {
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
            drag_ended(ivars.batch.as_deref());
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

impl Source {
    fn new(
        app: AppHandle,
        problems: Vec<String>,
        batch: Option<PathBuf>,
        mtm: MainThreadMarker,
    ) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(SourceIvars {
            app,
            problems,
            batch,
        });
        // SAFETY: plain NSObject init of a freshly allocated instance.
        unsafe { msg_send![super(this), init] }
    }
}
