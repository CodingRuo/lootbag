import type { PointerEvent } from "react";
import { invoke } from "@tauri-apps/api/core";
import { native } from "./ui.tsx";

const THRESHOLD = 6;
let origin: { x: number; y: number } | null = null;
let outgoing = false;

/** True while a card is being dragged out, so the import drop zone ignores Lootbag's own file. */
export const draggingOut = () => outgoing;

// WKWebView ignores HTML drag data, so a pointer drag past a few pixels starts a native copy drag in Rust.
export function dragOutHandlers(fileId: number, disabled: boolean, onError: (error: unknown) => void) {
  return {
    onDragStart: (event: { preventDefault(): void }) => event.preventDefault(),
    onPointerDown: (event: PointerEvent) => { origin = native && !disabled && event.button === 0 ? { x: event.clientX, y: event.clientY } : null; },
    onPointerUp: () => { origin = null; },
    onPointerMove: (event: PointerEvent) => {
      if (!origin || outgoing || !(event.buttons & 1)) return;
      if (Math.hypot(event.clientX - origin.x, event.clientY - origin.y) < THRESHOLD) return;
      origin = null; outgoing = true;
      invoke<boolean>("drag_file", { fileId }).catch(onError).finally(() => { outgoing = false; });
    },
  };
}
