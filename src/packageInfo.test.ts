import assert from "node:assert/strict";
import test from "node:test";
import { originFacts, savePackageInfo, type PackageInfo } from "./packageInfo.ts";

const empty: PackageInfo = { store: null, url: null, license: null, author: null, purchased: null, version: null, notes: null };

test("the package info dialog saves every field through set_package_info, empty inputs as null", async () => {
  const calls: unknown[] = [];
  await savePackageInfo(async (command, args) => { calls.push([command, args]); return undefined as never; }, 4,
    { store: " itch.io ", url: "https://x.io", license: "CC0", author: "", purchased: "2025-01-31", version: "", notes: "Bundle" });
  assert.deepEqual(calls, [["set_package_info", { packageId: 4, info: { store: "itch.io", url: "https://x.io", license: "CC0", author: null, purchased: "2025-01-31", version: null, notes: "Bundle" } }]]);
});

test("the origin tab lists shop, link, licence and author only when set", () => {
  assert.deepEqual(originFacts(undefined), []);
  assert.deepEqual(originFacts(empty), []);
  const facts = originFacts({ ...empty, store: "Kenney", license: "CC0", url: "https://kenney.nl", notes: "intern" });
  assert.deepEqual(facts.map((fact) => [fact.label, fact.value]), [["Shop", "Kenney"], ["Link", "https://kenney.nl"], ["Lizenz", "CC0"]]);
});
