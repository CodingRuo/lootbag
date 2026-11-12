import assert from "node:assert/strict";
import test from "node:test";
import { nextInspectorTab } from "./inspectorTabs.ts";

test("arrow navigation moves between inspector panels and wraps at both ends", () => {
  assert.equal(nextInspectorTab("preview", "ArrowRight"), "properties");
  assert.equal(nextInspectorTab("properties", "ArrowRight"), "origin");
  assert.equal(nextInspectorTab("origin", "ArrowRight"), "preview");
  assert.equal(nextInspectorTab("preview", "ArrowLeft"), "origin");
  assert.equal(nextInspectorTab("origin", "ArrowLeft"), "properties");
  assert.equal(nextInspectorTab("properties", "ArrowLeft"), "preview");
});

test("Home and End select the outer panels without consuming unrelated keys", () => {
  assert.equal(nextInspectorTab("origin", "Home"), "preview");
  assert.equal(nextInspectorTab("preview", "End"), "origin");
  assert.equal(nextInspectorTab("properties", "ArrowDown"), null);
  assert.equal(nextInspectorTab("properties", "Enter"), null);
  assert.equal(nextInspectorTab("preview", "Tab"), null);
});
