import assert from "node:assert/strict";
import test from "node:test";
import { answerDragQuery, dragOutHandlers, draggingOut, watchNativeDragOut, type DragOutEnd, type DragPlatform } from "./dragOut.ts";
import { dragSources } from "./catalogSelection.ts";
import type { DragSource } from "./catalogSelection.ts";

type Call = [string, Record<string, unknown> | undefined];
function setup(platform: DragPlatform, disabled = false, sources: () => DragSource[] = () => [{ fileId: 7 }], calls: Call[] = []) {
  const handlers = dragOutHandlers({
    sources, disabled, platform, onEnd: () => {}, onError: () => {},
    call: async <T,>(command: string, args?: Record<string, unknown>) => { calls.push([command, args]); return { dropped: true, problems: [] } as T; },
  });
  return { calls, handlers };
}
const press = { button: 0, clientX: 0, clientY: 0 };
const move = { buttons: 1, clientX: 20, clientY: 0 };

test("on macOS and in the browser preview JS never starts a drag; secondary buttons prepare nothing", () => {
  for (const [platform, button] of [["mac", 0], ["other", 2], [null, 0]] as const) {
    const { calls, handlers } = setup(platform);
    handlers.onPointerDown({ ...press, button });
    handlers.onPointerMove(move);
    assert.deepEqual(calls, []);
  }
});

type Asset = Parameters<typeof dragSources>[0];
const asset = (id: number): Asset => ({ files: [{ id }] }) as unknown as Asset;
function page(cards: Record<string, { x: number; y: number }>) {
  const points: [number, number][] = [];
  const doc = {
    elementFromPoint(x: number, y: number) {
      points.push([x, y]);
      const key = Object.keys(cards).find((name) => cards[name].x === x && cards[name].y === y);
      // The hit is a child of the card; the card is found by its data attribute.
      return { closest: (selector: string) => selector === "[data-drag-card]" && key ? { getAttribute: (name: string) => name === "data-drag-card" ? key : null } : null };
    },
  };
  return { doc, points };
}

test("a press query answers with the card under the press point and the current selection", async () => {
  const { doc, points } = page({ a: { x: 10, y: 20 }, b: { x: 10, y: 80 } });
  const assets: Record<string, Asset> = { a: asset(1), b: asset(2) };
  let selected: Asset[] = [];
  const filesFor = (key: string) => assets[key] ? dragSources(assets[key], selected) : null;
  const calls: Call[] = [];
  const call = async <T,>(command: string, args?: Record<string, unknown>) => { calls.push([command, args]); return undefined as T; };

  await answerDragQuery({ pressId: 4, x: 10, y: 20 }, filesFor, call, doc);
  // The selection changed after an earlier press: the answer uses it as it is now.
  selected = [assets.a, assets.b];
  await answerDragQuery({ pressId: 5, x: 10, y: 20 }, filesFor, call, doc);
  // Not on a card, or a card the page no longer offers: nothing to drag.
  await answerDragQuery({ pressId: 6, x: 500, y: 500 }, filesFor, call, doc);
  await answerDragQuery({ pressId: 7, x: 10, y: 80 }, () => null, call, doc);
  assert.deepEqual(points, [[10, 20], [10, 20], [500, 500], [10, 80]]);
  assert.deepEqual(calls, [
    ["answer_drag_query", { pressId: 4, files: [{ fileId: 1 }] }],
    ["answer_drag_query", { pressId: 5, files: [{ fileId: 1 }, { fileId: 2 }] }],
    ["answer_drag_query", { pressId: 6, files: null }],
    ["answer_drag_query", { pressId: 7, files: null }],
  ]);
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
  const stop = await watchNativeDragOut((end) => ends.push(end), () => null, async (name, handler) => {
    handlers.set(name, handler as (event: { payload: unknown }) => void);
    return () => handlers.delete(name);
  });
  assert.ok(handlers.has("drag-out-query"));
  handlers.get("drag-out-started")!({ payload: null });
  assert.equal(draggingOut(), true);
  handlers.get("drag-out-ended")!({ payload: { dropped: false, problems: ["Rock.obj: fehlt"], error: null } });
  assert.equal(draggingOut(), false);
  assert.deepEqual(ends, [{ dropped: false, problems: ["Rock.obj: fehlt"], error: null }]);
  stop();
  assert.equal(handlers.size, 0);
});
