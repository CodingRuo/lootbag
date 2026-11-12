import assert from "node:assert/strict";
import test from "node:test";
import { assetKey, imageFit, materialSelection, prepareSpriteSequence, spriteSheetFrame, type ImageGroup, type ImageGroupSummary } from "./imageWorkflows.ts";

const group: ImageGroup = {
  workflow: "material", name: "Stone", errors: ["baseColor ist nicht eindeutig"],
  resources: { snapshotId: "snapshot", fileId: 1, format: "PNG", entryPath: "Stone_Base_Color.png", declaredPaths: [], errors: [], resources: [
    { relativePath: "Stone_Base_Color.png", path: "/library/Stone_Base_Color.png", sizeBytes: 100 },
    { relativePath: "Stone_basecolor.png", path: "/library/Stone_basecolor.png", sizeBytes: 100 },
    { relativePath: "Stone_Normal.png", path: "/library/Stone_Normal.png", sizeBytes: 100 },
  ] },
  members: [
    { assetId: 1, file: { id: 1, originalFilename: "Stone_Base_Color.png", available: true }, role: "baseColor", frameIndex: null, resourcePath: "Stone_Base_Color.png" },
    { assetId: 2, file: { id: 2, originalFilename: "Stone_basecolor.png", available: true }, role: "baseColor", frameIndex: null, resourcePath: "Stone_basecolor.png" },
    { assetId: 3, file: { id: 3, originalFilename: "Stone_Normal.png", available: true }, role: "normal", frameIndex: null, resourcePath: "Stone_Normal.png" },
  ],
};

test("ambiguous material maps need a choice; bundle includes exactly the selected originals", () => {
  assert.throws(() => materialSelection(group, { baseColor: "", normal: "Stone_Normal.png" }), /baseColor/);
  assert.deepEqual(materialSelection(group, { baseColor: "Stone_basecolor.png", normal: "Stone_Normal.png" }), ["Stone_basecolor.png", "Stone_Normal.png"]);
  assert.deepEqual(materialSelection(group, { baseColor: "none", normal: "Stone_Normal.png" }), ["Stone_Normal.png"]);
});

test("an unavailable map can be omitted but cannot become an export selection", () => {
  const unavailable: ImageGroup = {
    ...group,
    members: group.members.map((member) => member.role === "normal" ? { ...member, file: { ...member.file, available: false } } : member),
    resources: { ...group.resources, errors: ["Stone_Normal.png: Quelldatei geändert"], resources: group.resources.resources.filter((resource) => resource.relativePath !== "Stone_Normal.png") },
  };
  assert.deepEqual(materialSelection(unavailable, { baseColor: "Stone_Base_Color.png", normal: "none" }), ["Stone_Base_Color.png"]);
  assert.throws(() => materialSelection(unavailable, { baseColor: "Stone_Base_Color.png", normal: "Stone_Normal.png" }), /nicht verfügbar/);
  assert.throws(() => materialSelection(group, { baseColor: "Stone_Normal.png" }), /nicht verfügbar/);
});

test("sprite playback uses numeric frame order and rejects duplicate indices and unequal dimensions", () => {
  const frames = [
    { index: 10, path: "water10.png", width: 128, height: 128 },
    { index: 2, path: "water2.png", width: 128, height: 128 },
    { index: 1, path: "water1.png", width: 128, height: 128 },
  ];
  assert.deepEqual(prepareSpriteSequence(frames).map((frame) => frame.path), ["water1.png", "water2.png", "water10.png"]);
  assert.throws(() => prepareSpriteSequence([...frames, { index: 1, path: "water01.png", width: 128, height: 128 }]), /doppelt/);
  assert.throws(() => prepareSpriteSequence([frames[0], { index: 2, path: "water2.png", width: 64, height: 128 }]), /Bildgröße/);
});

test("a large image fits the actual preview height and width with room for its padding", () => {
  assert.equal(imageFit(2048, 2048, 528, 1040), 0.25);
  assert.equal(imageFit(2048, 2048, 1040, 528), 0.25);
  assert.equal(imageFit(128, 128, 500, 500), 1);
});

test("manually shared asset records keep separate catalog selections for each material group", () => {
  const first: ImageGroupSummary = { workflow: "material", fileId: 10, name: "Stone", fileCount: 2, errors: [] };
  const second: ImageGroupSummary = { ...first, fileId: 20, name: "Wood" };
  assert.notEqual(assetKey({ id: 1, imageGroup: first }), assetKey({ id: 1, imageGroup: second }));
  assert.notEqual(assetKey({ id: 10 }), assetKey({ id: 1, imageGroup: first }));
});


test("sprite frame count is bounded even when every frame is tiny", () => {
  const frames = Array.from({ length: 257 }, (_, index) => ({ index, path: `Run_${index}.tga`, width: 1, height: 1 }));
  assert.throws(() => prepareSpriteSequence(frames), /256/);
});


const spacedSheet = { columns: 3, rows: 2, left: 2, top: 3, right: 3, bottom: 2, gapX: 1, gapY: 2 };

test("spritesheet margins and gutters crop the six independently specified rectangles", () => {
  assert.deepEqual(Array.from({ length: 6 }, (_, index) => spriteSheetFrame(25, 17, spacedSheet, index)), [
    { x: 2, y: 3, width: 6, height: 5, count: 6 },
    { x: 9, y: 3, width: 6, height: 5, count: 6 },
    { x: 16, y: 3, width: 6, height: 5, count: 6 },
    { x: 2, y: 10, width: 6, height: 5, count: 6 },
    { x: 9, y: 10, width: 6, height: 5, count: 6 },
    { x: 16, y: 10, width: 6, height: 5, count: 6 },
  ]);
});

test("invalid spritesheet inputs never return a rounded or out-of-image crop", () => {
  const invalid = [
    { ...spacedSheet, columns: 0 }, { ...spacedSheet, rows: 0 },
    { ...spacedSheet, columns: 1.5 }, { ...spacedSheet, rows: Number.POSITIVE_INFINITY },
    { ...spacedSheet, left: -1 }, { ...spacedSheet, top: 3.5 },
    { ...spacedSheet, right: Number.NaN }, { ...spacedSheet, bottom: -1 },
    { ...spacedSheet, gapX: -1 }, { ...spacedSheet, gapY: 1.5 },
    { ...spacedSheet, left: 25 }, { ...spacedSheet, top: 17 },
    { ...spacedSheet, gapX: 20 }, { ...spacedSheet, gapY: 20 },
    { ...spacedSheet, right: 2 }, { ...spacedSheet, bottom: 1 },
  ];
  for (const layout of invalid) assert.equal(spriteSheetFrame(25, 17, layout, 0), null);
  for (const index of [-1, 0.5, 6, Number.NaN, Number.MAX_SAFE_INTEGER + 1]) assert.equal(spriteSheetFrame(25, 17, spacedSheet, index), null);
  for (const [width, height] of [[0, 17], [25, 0], [25.5, 17], [25, Number.POSITIVE_INFINITY], [Number.MAX_SAFE_INTEGER + 1, 17]]) {
    assert.equal(spriteSheetFrame(width, height, spacedSheet, 0), null);
  }
});

test("zero defaults preserve the whole sheet and the existing edge-to-edge grid", () => {
  const zero = { columns: 1, rows: 1, left: 0, top: 0, right: 0, bottom: 0, gapX: 0, gapY: 0 };
  assert.deepEqual(spriteSheetFrame(12, 8, zero, 0), { x: 0, y: 0, width: 12, height: 8, count: 1 });
  assert.deepEqual(spriteSheetFrame(12, 8, { ...zero, columns: 3, rows: 2 }, 5), { x: 8, y: 4, width: 4, height: 4, count: 6 });
  assert.deepEqual(spriteSheetFrame(25, 17, { ...spacedSheet, columns: 1, rows: 1 }, 0), { x: 2, y: 3, width: 20, height: 12, count: 1 });
  assert.deepEqual(spriteSheetFrame(1024, 512, { ...zero, columns: 512, rows: 512 }, 262143), { x: 1022, y: 511, width: 2, height: 1, count: 262144 });
  assert.equal(spriteSheetFrame(Number.MAX_SAFE_INTEGER, Number.MAX_SAFE_INTEGER, { ...zero, columns: Number.MAX_SAFE_INTEGER, rows: Number.MAX_SAFE_INTEGER }, 0), null);
});
