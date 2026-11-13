//! macOS drag-out owned by Lootbag: a local event monitor sees the real mouse events, and a
//! payload staged for the held press starts `beginDraggingSession` from a real
//! `leftMouseDragged` event of that press, as AppKit requires. A press that ends first never
//! starts a drag.
//!
//! Press identity: the monitor records each mouse-down with its window and its point in the
//! pressed webview's viewport. Once the held press passes the threshold, that window's page is
//! asked (`drag-out-query`) which card lies under the press point; its DOM is still the one the
//! user pressed. Only the page's answer for that same, still held press is staged.

use super::{discard_stage, stage_drag};
use crate::library::{css_point, DragGesture, DragItem, DragSource, DragStage, DragStep};
use block2::RcBlock;
use objc2::{
    define_class, msg_send,
    rc::Retained,
    runtime::{AnyClass, ProtocolObject},
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
    ptr::NonNull,
    sync::{LazyLock, Mutex, MutexGuard, PoisonError},
};
use tauri::{AppHandle, Emitter, Manager};

/// A staged payload, or the German reason it could not be prepared (shown only if dragged).
pub type Prepared = Result<DragStage, String>;

static GESTURE: LazyLock<Mutex<DragGesture<Prepared>>> = LazyLock::new(Mutex::default);

thread_local! {
    // The running session's source; kept alive here until the next drag replaces it.
    static SOURCE: RefCell<Option<Retained<Source>>> = const { RefCell::new(None) };
}

fn gesture() -> MutexGuard<'static, DragGesture<Prepared>> {
    GESTURE.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The page's answer for `press` in `window` (an NSWindow pointer); stages it only if it is the
/// first answer for that window's still held press.
pub fn answer(app: AppHandle, window: usize, press: u64, files: Option<Vec<DragSource>>) {
    if !gesture().answer(window, press, &files) {
        return;
    }
    let Some(sources) = files else { return };
    tauri::async_runtime::spawn(async move {
        let prepared = stage_drag(app, sources).await;
        let refused = gesture().arm(press, prepared).err();
        discard(refused);
    });
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct DragQuery {
    press_id: u64,
    x: f64,
    y: f64,
}

/// Asks the webview window behind `window` which card lies under `at`.
fn query(app: &AppHandle, window: usize, press: u64, (x, y): (f64, f64)) {
    let target = app
        .webview_windows()
        .into_values()
        .find(|webview| webview.ns_window().is_ok_and(|ns| ns as usize == window));
    if let Some(target) = target {
        let payload = DragQuery {
            press_id: press,
            x,
            y,
        };
        let _ = target.emit_to(target.label(), "drag-out-query", payload);
    }
}

pub fn disarm() {
    discard(gesture().disarm());
}

/// Queues the copies of a payload no drag will use for removal; never blocks the event loop.
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
                let css = window
                    .and_then(|window| window.contentView())
                    .and_then(|view| webview_point(&view, at));
                let dropped = gesture().press(id, (at.x, at.y), css);
                discard(dropped);
            }
            // Any key while held (Escape, Cmd+A, arrows) may change the selection: never drag stale.
            NSEventType::LeftMouseUp | NSEventType::KeyDown => disarm(),
            NSEventType::LeftMouseDragged => {
                let step = gesture().drag((at.x, at.y));
                match step {
                    DragStep::Wait => {}
                    DragStep::Query { press, window, at } => query(&app, window, press, at),
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

/// The press as a CSS pixel in the viewport of the webview under it, if it hit one.
fn webview_point(content: &NSView, at: NSPoint) -> Option<(f64, f64)> {
    let class = AnyClass::get(c"WKWebView")?;
    let mut view = content.hitTest(at);
    while let Some(current) = view {
        if current.isKindOfClass(class) {
            let local = current.convertPoint_fromView(at, None);
            let height = current.bounds().size.height;
            return Some(css_point((local.x, local.y), height, current.isFlipped()));
        }
        // SAFETY: walking up the view tree of a live window on the main thread.
        view = unsafe { current.superview() };
    }
    None
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
    // The batch stays until the next launch: the end callback does not mean the recipient is done.
    let source = Source::new(app.clone(), stage.problems, mtm);
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
    fn new(app: AppHandle, problems: Vec<String>, mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(SourceIvars { app, problems });
        // SAFETY: plain NSObject init of a freshly allocated instance.
        unsafe { msg_send![super(this), init] }
    }
}
