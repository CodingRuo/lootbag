import assert from "node:assert/strict";
import test from "node:test";
import { catalogSelection, nextCatalogIndex } from "./catalogSelection.ts";

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
