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

/** Rust asks which card lies under a held press (viewport CSS pixels) once it starts moving. */
export type DragQuery = { pressId: number; x: number; y: number };
type QueryDocument = { elementFromPoint(x: number, y: number): { closest(selector: string): { getAttribute(name: string): string | null } | null } | null };

/** Answers a press query from the DOM under the still held press and the current selection. */
export function answerDragQuery({ pressId, x, y }: DragQuery, filesFor: (cardKey: string) => DragSource[] | null,
  call: Invoke = invoke, doc: QueryDocument = document) {
  const key = doc.elementFromPoint(x, y)?.closest("[data-drag-card]")?.getAttribute("data-drag-card");
  const files = key == null ? null : filesFor(key);
  return call("answer_drag_query", { pressId, files }).catch(() => {});
}

// WKWebView ignores HTML drag data. On macOS Rust owns the gesture: once a held press moves it asks
// the page which card lies under the press (`watchNativeDragOut`), stages that answer and starts
// the native drag from it; skipped items come back as `problems`.
export function dragOutHandlers({ sources, disabled, platform, onEnd, onError, call = invoke }: {
  sources: () => DragSource[]; disabled: boolean; platform: DragPlatform;
  onEnd: (end: DragOutEnd) => void; onError: (error: unknown) => void; call?: Invoke;
}) {
  return {
    onDragStart: (event: { preventDefault(): void }) => event.preventDefault(),
    onPointerDown: (event: Pick<PointerEvent, "button" | "clientX" | "clientY">) => {
      origin = null;
      if (platform === "other" && !disabled && event.button === 0) origin = { x: event.clientX, y: event.clientY };
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

/** macOS: answers press queries and follows the native session's start and end; returns the unsubscribe. */
export async function watchNativeDragOut(onEnd: (end: DragOutEnd) => void, filesFor: (cardKey: string) => DragSource[] | null,
  on: Listen = listen, call: Invoke = invoke) {
  const stops = await Promise.all([
    on<DragQuery>("drag-out-query", ({ payload }) => { answerDragQuery(payload, filesFor, call); }),
    on("drag-out-started", () => { outgoing = true; }),
    on<DragOutEnd>("drag-out-ended", ({ payload }) => { outgoing = false; onEnd(payload); }),
  ]);
  return () => stops.forEach((stop) => stop());
}
