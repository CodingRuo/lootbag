import type { PointerEvent } from "react";
import { invoke } from "@tauri-apps/api/core";
import { native } from "./ui.tsx";
import type { DragSource } from "./catalogSelection.ts";

const THRESHOLD = 6;
let origin: { x: number; y: number } | null = null;
let outgoing = false;

/** True while a card is being dragged out, so the import drop zone ignores Lootbag's own file. */
export const draggingOut = () => outgoing;

// WKWebView ignores HTML drag data, so a pointer drag past a few pixels starts a native copy drag in Rust.
// Staged copies and model bundles are prepared before the drag starts; skipped items come back as `problems`.
export function dragOutHandlers(sources: () => DragSource[], disabled: boolean, onProblems: (problems: string[]) => void, onError: (error: unknown) => void) {
  return {
    onDragStart: (event: { preventDefault(): void }) => event.preventDefault(),
    onPointerDown: (event: PointerEvent) => { origin = native && !disabled && event.button === 0 ? { x: event.clientX, y: event.clientY } : null; },
    onPointerUp: () => { origin = null; },
    onPointerMove: (event: PointerEvent) => {
      if (!origin || outgoing || !(event.buttons & 1)) return;
      if (Math.hypot(event.clientX - origin.x, event.clientY - origin.y) < THRESHOLD) return;
      origin = null; outgoing = true;
      invoke<{ dropped: boolean; problems: string[] }>("drag_assets", { sources: sources() })
        .then(({ problems }) => { if (problems.length) onProblems(problems); }).catch(onError).finally(() => { outgoing = false; });
    },
  };
}
