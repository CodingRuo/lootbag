import type { PointerEvent } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { DragSource } from "./catalogSelection.ts";

type Invoke = <T>(command: string, args?: Record<string, unknown>) => Promise<T>;
type Listen = <T>(event: string, handler: (event: { payload: T }) => void) => Promise<() => void>;
/** How a drag-out ended; `error` means nothing could be prepared. */
export type DragOutEnd = { dropped: boolean; problems: string[]; error?: string | null };
/** `mac`: Rust owns the gesture natively; `other`: the drag starts from pointer moves; null: no native shell. */
export type DragPlatform = "mac" | "other" | null;

const THRESHOLD = 6;
let origin: { x: number; y: number } | null = null;
let outgoing = false;

/** True while a card is being dragged out, so the import drop zone ignores Lootbag's own file. */
export const draggingOut = () => outgoing;

type CardElement = { getBoundingClientRect(): { left: number; top: number; width: number; height: number }; isConnected: boolean };
type Card = { sources: () => DragSource[]; disabled: boolean; call: Invoke };
// The latest render's props per card element, so a selection change re-reports fresh sources.
const cards = new WeakMap<CardElement, Card>();
let hovered: CardElement | null = null;
let reported = "null";
let version = 0;

/** Tells Rust which card is under the pointer and where (viewport CSS pixels); only on change. */
function report(element: CardElement | null, fallback: Invoke) {
  const card = element ? cards.get(element) : undefined;
  let target = null;
  if (element && card && !card.disabled && element.isConnected) {
    const { left, top, width, height } = element.getBoundingClientRect();
    target = { left, top, width, height, sources: card.sources() };
  }
  const key = JSON.stringify(target);
  if (key === reported) return;
  reported = key;
  // Time-based, so a reloaded page still outranks its own earlier reports in Rust.
  version = Math.max(version + 1e-3, performance.timeOrigin + performance.now());
  (card?.call ?? fallback)("report_drag_target", { version, target }).catch(() => {});
}

/** macOS: re-reports the hovered card after a render, scroll or selection change. */
export function refreshDragTarget(call: Invoke = invoke) {
  const card = hovered ? cards.get(hovered) : undefined;
  if (hovered && !hovered.isConnected) hovered = null;
  report(hovered, card?.call ?? call);
}

// WKWebView ignores HTML drag data. On macOS Rust owns the gesture: the card under the pointer is
// reported ahead of any press, Rust binds it to a native press inside its rect, stages it once that
// press moves and starts the native drag from it; skipped items come back as `problems`.
export function dragOutHandlers({ sources, disabled, platform, onEnd, onError, call = invoke }: {
  sources: () => DragSource[]; disabled: boolean; platform: DragPlatform;
  onEnd: (end: DragOutEnd) => void; onError: (error: unknown) => void; call?: Invoke;
}) {
  const hover = (event: { currentTarget: CardElement }) => {
    if (platform !== "mac") return;
    hovered = event.currentTarget;
    report(hovered, call);
  };
  return {
    ref: (element: CardElement | null) => { if (element && platform === "mac") cards.set(element, { sources, disabled, call }); },
    onDragStart: (event: { preventDefault(): void }) => event.preventDefault(),
    onPointerEnter: hover,
    onPointerLeave: (event: { currentTarget: CardElement }) => {
      if (platform !== "mac" || hovered !== event.currentTarget) return;
      hovered = null;
      report(null, call);
    },
    onPointerDown: (event: Pick<PointerEvent, "button" | "clientX" | "clientY">) => {
      origin = null;
      if (platform === "other" && !disabled && event.button === 0) origin = { x: event.clientX, y: event.clientY };
    },
    onPointerUp: () => { origin = null; },
    onPointerMove: (event: Pick<PointerEvent, "buttons" | "clientX" | "clientY"> & { currentTarget: CardElement }) => {
      hover(event);
      if (!origin || outgoing || !(event.buttons & 1)) return;
      if (Math.hypot(event.clientX - origin.x, event.clientY - origin.y) < THRESHOLD) return;
      origin = null; outgoing = true;
      call<DragOutEnd>("drag_assets", { sources: sources() }).then(onEnd).catch(onError).finally(() => { outgoing = false; });
    },
  };
}

/** macOS: the native session reports its start and end; returns the unsubscribe. */
export async function watchNativeDragOut(onEnd: (end: DragOutEnd) => void, on: Listen = listen) {
  const stops = await Promise.all([
    on("drag-out-started", () => { outgoing = true; }),
    on<DragOutEnd>("drag-out-ended", ({ payload }) => { outgoing = false; onEnd(payload); }),
  ]);
  return () => stops.forEach((stop) => stop());
}
