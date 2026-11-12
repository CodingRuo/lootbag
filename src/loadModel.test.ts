import assert from "node:assert/strict";
import test from "node:test";
import { Bone, BoxGeometry, BufferAttribute, DataTexture, Group, Mesh, MeshStandardMaterial, PerspectiveCamera, Points, Texture, Vector3 } from "three";
import { TGALoader } from "three/addons/loaders/TGALoader.js";
import { disposeObjects, fitModelCamera, modelScene, modelTextureAvailable, modelTransportUrl, type LoadedModel } from "./loadModel.ts";

test("TGA textures with missing decoded pixels cannot silently authorize complete export", () => {
  assert.equal(modelTextureAvailable(new DataTexture()), false);
  assert.equal(modelTextureAvailable(new DataTexture(new Uint8Array(), 1, 1)), false);
  const neutral = Uint8Array.from([0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 1, 0, 32, 40, 189, 185, 181, 255]);
  const decoded = new TGALoader().parse(neutral.buffer);
  assert.equal(modelTextureAvailable(new DataTexture(decoded.data, decoded.width, decoded.height)), true);
  assert.equal(modelTextureAvailable(new Texture({ width: 2, height: 2 })), true);
});

test("snapshot transports isolate duplicate loader keys without changing local paths or embedded data", async () => {
  const original = "asset://localhost/package/Models/a%20b.fbx";
  const first = modelTransportUrl(original, "inspector-1");
  const second = modelTransportUrl(original, "thumbnail-2");
  assert.notEqual(first, second);
  assert.equal(new URL(first).pathname, new URL(original).pathname);
  assert.equal(new URL(second).pathname, new URL(original).pathname);
  const embedded = "data:application/octet-stream;base64,AAECA/8=";
  const data = await fetch(modelTransportUrl(embedded, "thumbnail-3"));
  assert.deepEqual([...new Uint8Array(await data.arrayBuffer())], [0, 1, 2, 3, 255]);
});

test("the shared preview camera contains transformed geometry at catalog and inspector aspect ratios", () => {
  const model = new Group();
  const mesh = new Mesh(new BoxGeometry(8, 2, 4), new MeshStandardMaterial());
  mesh.position.set(100, -50, 25);
  mesh.rotation.set(0.3, 0.5, 0.7);
  model.add(mesh);
  for (const aspect of [192 / 144, 0.5, 2]) {
    const camera = new PerspectiveCamera(45, aspect, 0.01, 1000);
    fitModelCamera(camera, model);
    camera.updateMatrixWorld();
    model.updateMatrixWorld(true);
    const positions = mesh.geometry.getAttribute("position");
    for (let index = 0; index < positions.count; index++) {
      const projected = new Vector3().fromBufferAttribute(positions, index).applyMatrix4(mesh.matrixWorld).project(camera);
      assert.ok(Math.abs(projected.x) < 1 && Math.abs(projected.y) < 1 && Math.abs(projected.z) < 1);
    }
  }
  disposeObjects([model]);
});

test("shared geometry, materials and texture resources are released exactly once", () => {
  const geometry = new BoxGeometry();
  const texture = new Texture();
  const material = new MeshStandardMaterial({ map: texture });
  let geometries = 0;
  let materials = 0;
  let textures = 0;
  geometry.addEventListener("dispose", () => { geometries++; });
  material.addEventListener("dispose", () => { materials++; });
  texture.addEventListener("dispose", () => { textures++; });
  const root = new Group();
  root.add(new Mesh(geometry, [material, material]), new Mesh(geometry, material));
  disposeObjects([root], [material]);
  assert.equal(geometries, 1);
  assert.equal(materials, 1);
  assert.equal(textures, 1);
});

test("the shared scene keeps skeleton-only models visible and rejects a degenerate skeleton", () => {
  const model = new Group();
  const root = new Bone();
  const tip = new Bone();
  tip.position.set(0, 2, 0);
  root.add(tip);
  model.add(root);
  const loaded: LoadedModel = {
    model, clips: [], stats: { meshes: 0, vertices: 0, triangles: 0, bones: 2 },
    warnings: [], replacements: [], resourcePaths: [], staticGifTextures: false, staticAsepriteTextures: false, dispose: () => {},
  };
  const view = modelScene(loaded);
  assert.ok(view.scene.children.some((child) => child.type === "SkeletonHelper"));
  const joints = view.scene.children.find((child) => child instanceof Points)!;
  model.updateMatrixWorld(true);
  (joints.onBeforeRender as () => void)();
  assert.deepEqual(new Vector3().fromBufferAttribute(joints.geometry.getAttribute("position") as BufferAttribute, 1).toArray(), [0, 2, 0]);
  const camera = new PerspectiveCamera(45, 192 / 144, 0.01, 1000);
  fitModelCamera(camera, model);
  assert.ok(camera.position.toArray().every(Number.isFinite));
  view.dispose();
  tip.position.set(0, 0, 0);
  assert.throws(() => modelScene(loaded), /Gelenkabstände/);
});

test("a crouching skeleton fills most of the frame instead of its loose bounding sphere", () => {
  const model = new Group();
  const joints = [[0, 0, 0], [15, 58, -6], [6, 94, 19], [3, 106, 58], [63, 28, 43], [-39, 37, -22], [-11, 20, -41], [20, 0, 10]]
    .map(([x, y, z]) => { const bone = new Bone(); bone.position.set(x, y, z); model.add(bone); return bone; });
  const camera = new PerspectiveCamera(45, 1.45, 0.01, 1000);
  fitModelCamera(camera, model);
  camera.updateMatrixWorld();
  model.updateMatrixWorld(true);
  const ys = joints.map((bone) => {
    const projected = bone.getWorldPosition(new Vector3()).project(camera);
    assert.ok(Math.abs(projected.x) <= 0.71 && Math.abs(projected.y) <= 0.71);
    return projected.y;
  });
  const height = (Math.max(...ys) - Math.min(...ys)) / 2;
  assert.ok(height > 0.55 && height < 0.75, `skeleton fills ${height} of the viewport height`);
});
