import assert from "node:assert/strict";
import test from "node:test";
import { resolveModelResource, type ModelResource } from "./modelResources.ts";

const resources = (...paths: string[]): ModelResource[] => paths.map((relativePath) => ({ relativePath, path: `/package/${relativePath}`, sizeBytes: 1 }));

test("exact package path wins over duplicate basenames and replacement candidates", () => {
  const items = resources("Textures/Palette.psd", "Other/Palette.psd", "Textures/Palette.png");
  assert.deepEqual(resolveModelResource("Textures/Palette.psd", "FBX", items), { resource: items[0], substituted: false });
});

test("existing FBX basename resolution remains case-insensitive and unique", () => {
  const items = resources("Textures/Palette.PNG");
  assert.deepEqual(resolveModelResource("Models/palette.png", "FBX", items), { resource: items[0], substituted: false });
  assert.throws(() => resolveModelResource("Models/palette.png", "FBX", resources("A/palette.png", "B/Palette.PNG")), /nicht eindeutig/);
});

test("PSD can use one supported image with exactly the same normalized stem", () => {
  for (const extension of ["PNG", "jpg", "jpeg", "webp"]) {
    const items = resources(`Textures/Pa\u0308lette.${extension}`, "Textures/Other.png");
    assert.deepEqual(resolveModelResource("Models/PÄLETTE.PSD", "FBX", items), { resource: items[0], substituted: true });
  }
});

test("ambiguous PSD replacements fail regardless of path or extension", () => {
  for (const paths of [["A/palette.png", "B/Palette.PNG"], ["Textures/palette.png", "Textures/palette.jpg"]]) {
    assert.throws(() => resolveModelResource("Models/palette.psd", "FBX", resources(...paths)), /Ersatztextur ist nicht eindeutig/);
  }
});

test("a missing FBX PSD does not abort geometry or guess a differently named texture", () => {
  for (const paths of [[], ["Textures/Other.png"], ["Textures/palette.tga"]]) {
    assert.deepEqual(resolveModelResource("Models/palette.psd", "FBX", resources(...paths)), {
      resource: null, substituted: false, missingTexture: "Models/palette.psd",
    });
  }
  const reference = "FBX/Characters/Unreal_Characters/PolygonApocalypse_Texture_01_Cleaned.psd";
  assert.deepEqual(resolveModelResource(reference, "FBX", resources("Textures/PolygonApocalypse_Texture_01_A.png")), {
    resource: null, substituted: false, missingTexture: reference,
  });
});

test("missing non-FBX resources and unsupported replacements remain errors", () => {
  for (const format of ["GLTF", "GLB", "OBJ"]) {
    assert.throws(() => resolveModelResource("Models/palette.psd", format, resources("Textures/palette.png")), /fehlt/);
  }
  assert.throws(() => resolveModelResource("Models/palette.bin", "FBX", resources("Textures/palette.png")), /fehlt/);
});

test("missing known FBX images permit warned geometry while exact TGA resources retain precedence", () => {
  for (const extension of ["png", "jpg", "jpeg", "webp", "tga", "gif"]) {
    const relative = `Models/missing.${extension}`;
    assert.deepEqual(resolveModelResource(relative, "FBX", resources("Textures/unrelated.png")), {
      resource: null, substituted: false, missingTexture: relative,
    });
  }
  const items = resources("Textures/Misc/Wire_Alpha.tga");
  assert.deepEqual(resolveModelResource("Models/Wire_Alpha.tga", "FBX", items), { resource: items[0], substituted: false });
});

test("Blender's saved preview accepts only the stamped entry and never promises a model bundle", async () => {
  const { blendEntryResource, requiresModelBundle } = await import("./modelResources.ts");
  const entry = { relativePath: "Scenes/Owned.blend", path: "/owned/Scenes/Owned.blend", sizeBytes: 128 };
  const snapshot = { format: "BLEND", entryPath: "Scenes/Owned.blend", declaredPaths: [], resources: [entry] };
  assert.equal(blendEntryResource(snapshot), entry);
  assert.equal(requiresModelBundle({ kind: "model", format: "BLEND" }), false);
  assert.equal(requiresModelBundle({ kind: "model", format: "FBX" }), true);
  assert.equal(requiresModelBundle({ kind: "image", format: "PNG" }), false);
  assert.throws(() => blendEntryResource({ ...snapshot, resources: [] }), /Originaldatei/);
  assert.throws(() => blendEntryResource({ ...snapshot, resources: [entry, { ...entry, relativePath: "Companion.png" }] }), /Originaldatei/);
  assert.throws(() => blendEntryResource({ ...snapshot, declaredPaths: ["Companion.png"] }), /Originaldatei/);
  assert.throws(() => blendEntryResource({ ...snapshot, resources: [{ ...entry, relativePath: "Other.blend" }] }), /Originaldatei/);
  assert.throws(() => blendEntryResource({ ...snapshot, resources: [{ ...entry, sizeBytes: 67108865 }] }), /64 MiB/);
});
