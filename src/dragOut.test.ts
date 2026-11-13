import assert from "node:assert/strict";
import test from "node:test";
import { dragOutHandlers, draggingOut, refreshDragTarget, watchNativeDragOut, type DragOutEnd, type DragPlatform } from "./dragOut.ts";
import type { DragSource } from "./catalogSelection.ts";

type Call = [string, Record<string, unknown> | undefined];
function setup(platform: DragPlatform, disabled = false, sources: () => DragSource[] = () => [{ fileId: 7 }], calls: Call[] = []) {
  const handlers = dragOutHandlers({
    sources, disabled, platform, onEnd: () => {}, onError: () => {},
    call: async <T,>(command: string, args?: Record<string, unknown>) => { calls.push([command, args]); return { dropped: true, problems: [] } as T; },
  });
  return { calls, handlers };
}
function element(left = 10, top = 20) {
  return { isConnected: true, getBoundingClientRect: () => ({ left, top, width: 100, height: 50 }) };
}
const press = { button: 0, clientX: 0, clientY: 0 };
const move = { buttons: 1, clientX: 20, clientY: 0, currentTarget: element() };
const targets = (calls: Call[]) => calls.map(([command, args]) => {
  assert.equal(command, "report_drag_target");
  return args?.target ?? null;
});

test("on macOS the hovered card is reported with its rect, only on change, and JS never starts a drag", () => {
  const card = element();
  const { calls, handlers } = setup("mac");
  handlers.ref(card);
  handlers.onPointerEnter({ currentTarget: card });
  handlers.onPointerMove({ ...move, currentTarget: card });
  handlers.onPointerDown(press);
  handlers.onPointerMove({ ...move, clientX: 40, currentTarget: card });
  handlers.onPointerLeave({ currentTarget: card });
  assert.deepEqual(targets(calls), [{ left: 10, top: 20, width: 100, height: 50, sources: [{ fileId: 7 }] }, null]);
  const versions = calls.map(([, args]) => args?.version as number);
  assert.ok(versions[1] > versions[0], "versions only grow");
});

test("a selection change or a moved card while hovering is reported again, a removed card clears", () => {
  const card = element();
  let selected: DragSource[] = [{ fileId: 7 }];
  const calls: Call[] = [];
  const first = setup("mac", false, () => selected, calls);
  first.handlers.ref(card);
  first.handlers.onPointerEnter({ currentTarget: card });
  // The next render's props (new selection) are registered by its ref; the effect refreshes.
  selected = [{ fileId: 7 }, { fileId: 9 }];
  setup("mac", false, () => selected, calls).handlers.ref(card);
  refreshDragTarget();
  refreshDragTarget();
  card.getBoundingClientRect = () => ({ left: 10, top: -80, width: 100, height: 50 });
  refreshDragTarget();
  card.isConnected = false;
  refreshDragTarget();
  assert.deepEqual(targets(calls), [
    { left: 10, top: 20, width: 100, height: 50, sources: [{ fileId: 7 }] },
    { left: 10, top: 20, width: 100, height: 50, sources: [{ fileId: 7 }, { fileId: 9 }] },
    { left: 10, top: -80, width: 100, height: 50, sources: [{ fileId: 7 }, { fileId: 9 }] },
    null,
  ]);
});

test("disabled cards report no target; secondary buttons and the browser preview prepare nothing", () => {
  const card = element(500, 0);
  const disabled = setup("mac", true);
  disabled.handlers.ref(card);
  disabled.handlers.onPointerEnter({ currentTarget: card });
  disabled.handlers.onPointerMove({ ...move, currentTarget: card });
  // Already cleared by the previous test, so a disabled card reports nothing new.
  assert.deepEqual(disabled.calls, []);
  for (const [platform, button] of [["other", 2], [null, 0]] as const) {
    const { calls, handlers } = setup(platform);
    handlers.ref(element());
    handlers.onPointerEnter({ currentTarget: element() });
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
