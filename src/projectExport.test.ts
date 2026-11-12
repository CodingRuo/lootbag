import assert from "node:assert/strict";
import test from "node:test";
import { exportProject, memberRole, saveFolderMember } from "./projectExport.ts";

test("role edits save the membership with its variant through set_folder_member", async () => {
  const calls: unknown[] = [];
  await saveFolderMember(async (command, args) => { calls.push([command, args]); return undefined as never; }, 7, { folderId: 3, fileId: 12, role: "Held" });
  assert.deepEqual(calls, [["set_folder_member", { folderId: 3, assetId: 7, fileId: 12, role: "Held" }]]);
});

test("export reports missing files before copying and copies only after confirmation", async () => {
  const calls: string[] = [];
  const invoke = async (command: string) => {
    calls.push(command);
    return (command === "plan_project_export" ? { project: "Game", assetCount: 3, sizeBytes: 10, missing: ["Gone (Pack): Datei nicht verfügbar"], substitutions: 0 } : { status: "exported", path: "/x/Game" }) as never;
  };
  let message = "";
  assert.equal(await exportProject(1, invoke, async () => "/x", async (text) => { message = text; return false; }), null);
  assert.match(message, /1 von 3 Assets.*\n• Gone \(Pack\)/);
  assert.deepEqual(calls, ["plan_project_export"]);
  assert.deepEqual(await exportProject(1, invoke, async () => "/x", async () => true), { status: "exported", path: "/x/Game" });
  assert.deepEqual(calls, ["plan_project_export", "plan_project_export", "export_project"]);
  assert.equal(await exportProject(1, invoke, async () => null, async () => true), null);
  assert.equal(calls.length, 3);
});

test("catalog role prefers the active folder and falls back to a subfolder membership", () => {
  const folders = [{ id: 1, parentId: null }, { id: 2, parentId: 1 }, { id: 3, parentId: null }];
  assert.equal(memberRole([{ folderId: 3, fileId: null, role: "Fremd" }, { folderId: 2, fileId: null, role: "Held" }], 1, folders), "Held");
  assert.equal(memberRole([{ folderId: 1, fileId: null, role: "Wald" }, { folderId: 2, fileId: null, role: "Held" }], 1, folders), "Wald");
  assert.equal(memberRole([{ folderId: 3, fileId: null, role: "Fremd" }], 1, folders), null);
});

test("export reports texture substitutions before copying", async () => {
  const plan = { project: "Game", assetCount: 1, sizeBytes: 10, missing: [], substitutions: 2 };
  let message = "";
  const calls: string[] = [];
  const invoke = async (command: string) => { calls.push(command); return plan as never; };
  assert.equal(await exportProject(1, invoke, async () => "/x", async (text) => { message = text; return false; }), null);
  assert.match(message, /^2 Texturen werden durch gleichnamige Dateien ersetzt/);
  assert.deepEqual(calls, ["plan_project_export"]);
});

test("export names packages without licence before copying and still exports on confirmation", async () => {
  const plan = { project: "Game", assetCount: 4, sizeBytes: 10, missing: [], substitutions: 0, packagesWithoutLicense: 3 };
  let message = "";
  const calls: string[] = [];
  const invoke = async (command: string) => { calls.push(command); return (command === "plan_project_export" ? plan : { status: "exported", path: "/x/Game" }) as never; };
  assert.deepEqual(await exportProject(1, invoke, async () => "/x", async (text) => { message = text; return true; }), { status: "exported", path: "/x/Game" });
  assert.match(message, /^3 Pakete ohne Lizenzangabe/);
  assert.deepEqual(calls, ["plan_project_export", "export_project"]);
});
