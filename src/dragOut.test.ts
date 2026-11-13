import assert from "node:assert/strict";
import test from "node:test";
import { dragOutHandlers, draggingOut, watchNativeDragOut, type DragOutEnd, type DragPlatform } from "./dragOut.ts";

function setup(platform: DragPlatform, disabled = false) {
  const calls: [string, unknown][] = [];
  const handlers = dragOutHandlers({
    sources: () => [{ fileId: 7 }], disabled, platform, onEnd: () => {}, onError: () => {},
    call: async <T,>(command: string, args?: Record<string, unknown>) => { calls.push([command, args]); return { dropped: true, problems: [] } as T; },
  });
  return { calls, handlers };
}
const press = { button: 0, clientX: 0, clientY: 0 };
const move = { buttons: 1, clientX: 20, clientY: 0 };

test("on macOS the press prepares the payload and moves never start a drag from JS", () => {
  const { calls, handlers } = setup("mac");
  handlers.onPointerDown(press);
  handlers.onPointerMove(move);
  assert.deepEqual(calls, [["prepare_drag", { sources: [{ fileId: 7 }] }]]);
});

test("secondary buttons, disabled cards and the browser preview prepare nothing", () => {
  for (const [platform, disabled, button] of [["mac", false, 2], ["mac", true, 0], [null, false, 0]] as const) {
    const { calls, handlers } = setup(platform, disabled);
    handlers.onPointerDown({ ...press, button });
    handlers.onPointerMove(move);
    assert.deepEqual(calls, []);
  }
});

test("elsewhere a held move past the threshold starts drag_assets once, a release first starts nothing", () => {
  const released = setup("other");
  released.handlers.onPointerDown(press);
  released.handlers.onPointerUp();
  released.handlers.onPointerMove(move);
  assert.deepEqual(released.calls, []);

  const { calls, handlers } = setup("other");
  handlers.onPointerDown(press);
  handlers.onPointerMove({ ...move, clientX: 3 });
  handlers.onPointerMove(move);
  handlers.onPointerMove(move);
  assert.deepEqual(calls, [["drag_assets", { sources: [{ fileId: 7 }] }]]);
});

test("native start and end events guard the import drop zone and report the outcome", async () => {
  const handlers = new Map<string, (event: { payload: unknown }) => void>();
  const ends: DragOutEnd[] = [];
  const stop = await watchNativeDragOut((end) => ends.push(end), async (name, handler) => {
    handlers.set(name, handler as (event: { payload: unknown }) => void);
    return () => handlers.delete(name);
  });
  handlers.get("drag-out-started")!({ payload: null });
  assert.equal(draggingOut(), true);
  handlers.get("drag-out-ended")!({ payload: { dropped: false, problems: ["Rock.obj: fehlt"], error: null } });
  assert.equal(draggingOut(), false);
  assert.deepEqual(ends, [{ dropped: false, problems: ["Rock.obj: fehlt"], error: null }]);
  stop();
  assert.equal(handlers.size, 0);
});
