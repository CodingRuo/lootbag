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

// WKWebView ignores HTML drag data. On macOS the press prepares the payload in Rust right away,
// and Rust's event monitor starts the native drag from that same held press; skipped items come back as `problems`.
export function dragOutHandlers({ sources, disabled, platform, onEnd, onError, call = invoke }: {
  sources: () => DragSource[]; disabled: boolean; platform: DragPlatform;
  onEnd: (end: DragOutEnd) => void; onError: (error: unknown) => void; call?: Invoke;
}) {
  return {
    onDragStart: (event: { preventDefault(): void }) => event.preventDefault(),
    onPointerDown: (event: Pick<PointerEvent, "button" | "clientX" | "clientY">) => {
      origin = null;
      if (!platform || disabled || event.button !== 0) return;
      if (platform === "mac") call("prepare_drag", { sources: sources() }).catch(onError);
      else origin = { x: event.clientX, y: event.clientY };
    },
    onPointerUp: () => { origin = null; },
    onPointerMove: (event: Pick<PointerEvent, "buttons" | "clientX" | "clientY">) => {
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
