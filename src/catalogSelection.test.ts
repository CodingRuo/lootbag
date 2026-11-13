import assert from "node:assert/strict";
import test from "node:test";
import { catalogSelection, dragSources, nextCatalogIndex, storeCatalogView, storedCatalogView } from "./catalogSelection.ts";

const keys = ["asset:1", "asset:2", "material:3", "asset:4"];

test("catalog selection supports individual toggles and an anchored range without hidden page selections", () => {
  let selection = catalogSelection({ keys: [], anchor: null }, keys, "asset:1", "single");
  selection = catalogSelection(selection, keys, "material:3", "toggle");
  assert.deepEqual(selection.keys, ["asset:1", "material:3"]);
  selection = catalogSelection(selection, keys, "asset:4", "range");
  assert.deepEqual(selection.keys, ["material:3", "asset:4"]);
  selection = catalogSelection(selection, keys, "material:3", "toggle");
  assert.deepEqual(selection.keys, ["asset:4"]);
  selection = catalogSelection(selection, ["asset:9"], "asset:9", "range");
  assert.deepEqual(selection.keys, ["asset:9"]);
});

test("keyboard navigation stays inside cards and list pages and ignores unrelated keys", () => {
  assert.equal(nextCatalogIndex(1, 7, "ArrowDown", 3), 4);
  assert.equal(nextCatalogIndex(4, 7, "ArrowUp", 3), 1);
  assert.equal(nextCatalogIndex(0, 7, "ArrowLeft", 3), 0);
  assert.equal(nextCatalogIndex(6, 7, "ArrowRight", 3), 6);
  assert.equal(nextCatalogIndex(2, 7, "ArrowDown", 1), 3);
  assert.equal(nextCatalogIndex(2, 7, "Home", 3), 0);
  assert.equal(nextCatalogIndex(2, 7, "End", 3), 6);
  assert.equal(nextCatalogIndex(2, 7, "Tab", 3), null);
});

test("the catalog view is remembered per type and falls back to cards when storage fails", () => {
  const values = new Map<string, string>();
  const storage = { getItem: (key: string) => values.get(key) ?? null, setItem: (key: string, value: string) => { values.set(key, value); } };
  assert.equal(storedCatalogView("animation", storage), "cards");
  storeCatalogView("animation", "list", storage);
  assert.equal(storedCatalogView("animation", storage), "list");
  assert.equal(storedCatalogView("sound", storage), "cards");
  assert.equal(storedCatalogView("", storage), "cards");
  const broken = { getItem: () => { throw new Error("blocked"); }, setItem: () => { throw new Error("blocked"); } };
  assert.doesNotThrow(() => storeCatalogView("animation", "list", broken));
  assert.equal(storedCatalogView("animation", broken), "cards");
});

test("dragging a selected card takes the whole selection, any other card only itself", () => {
  const sound = { id: 1, files: [{ id: 11 }, { id: 12 }] };
  const material = { id: 2, files: [{ id: 21 }], imageGroup: { workflow: "material" as const, fileId: 21, name: "Stone", fileCount: 2, errors: [] } };
  const other = { id: 3, files: [{ id: 31 }, { id: 32 }] };
  assert.deepEqual(dragSources(sound, [sound, material]), [{ fileId: 11 }, { fileId: 12 }, { fileId: 21, workflow: "material" }]);
  assert.deepEqual(dragSources(other, [sound, material]), [{ fileId: 31 }]);
  assert.deepEqual(dragSources(material, []), [{ fileId: 21, workflow: "material" }]);
});
