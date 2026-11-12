import { convertFileSrc } from "@tauri-apps/api/core";
import {
  Bone, Box3, BufferAttribute, BufferGeometry, Color, DataTexture, DirectionalLight, FileLoader, HemisphereLight, Line, LoadingManager,
  Material, Mesh, Object3D, PerspectiveCamera, Points, PointsMaterial, Scene, SkeletonHelper, Texture, Vector3,
  type AnimationClip,
} from "three";
import { FBXLoader } from "three/addons/loaders/FBXLoader.js";
import { GLTFLoader } from "three/addons/loaders/GLTFLoader.js";
import { MTLLoader } from "three/addons/loaders/MTLLoader.js";
import { OBJLoader } from "three/addons/loaders/OBJLoader.js";
import { ImageTexturePreviewLoader } from "./TgaPreviewLoader.ts";
import { maxGifBytes } from "./gifPreflight.ts";
import { resolveModelResource, type ModelResource } from "./modelResources.ts";
import { modelPreviewBounds } from "./modelAnimation.ts";
import { errorMessage } from "./errorMessage.ts";

export type ModelResources = {
  snapshotId: string;
  fileId: number;
  format: string;
  entryPath: string;
  resources: ModelResource[];
  declaredPaths: string[];
  errors: string[];
};

export function modelTransportUrl(url: string, snapshotId: string) {
  // Three deduplicates FileLoader URLs globally; cancellation must stay local to this snapshot.
  const transport = new URL(url);
  transport.hash = `model-preview=${encodeURIComponent(snapshotId)}`;
  return transport.href;
}

export function disposeObjects(objects: Object3D[], extraMaterials: Material[] = []) {
  const geometries = new Set<Mesh["geometry"]>();
  const materials = new Set(extraMaterials);
  const textures = new Set<Texture>();
  const images = new Set<ImageBitmap>();
  for (const object of objects) object.traverse((child) => {
    if (child instanceof Mesh || child instanceof Line || child instanceof Points) {
      geometries.add(child.geometry);
      for (const material of Array.isArray(child.material) ? child.material : [child.material]) materials.add(material);
    }
  });
  for (const material of materials) {
    for (const value of Object.values(material)) if (value instanceof Texture) textures.add(value);
    material.dispose();
  }
  for (const texture of textures) {
    const image = texture.source.data;
    if (typeof ImageBitmap !== "undefined" && image instanceof ImageBitmap) images.add(image);
    texture.dispose();
  }
  for (const image of images) image.close();
  for (const geometry of geometries) geometry.dispose();
}

export function modelTextureAvailable(texture: Texture<{ width: number; height: number }>) {
  const image = texture.image;
  return Boolean(image?.width && image?.height && (!(texture instanceof DataTexture) || texture.image.data?.length));
}

export async function loadModel(snapshot: ModelResources, signal: AbortSignal) {
  const manager = new LoadingManager();
  const objects: Object3D[] = [];
  const materials: Material[] = [];
  const blobs = new Set<string>();
  const warnings = [...snapshot.errors];
  const replacements: string[] = [];
  let staticGifTextures = false;
  let staticAsepriteTextures = false;
  const used = new Set(snapshot.declaredPaths);
  const warn = (message: string) => { if (!warnings.includes(message)) warnings.push(message); };
  const textureBudget = { pixels: 0 };
  manager.addHandler(/\.tga(?:$|[?#])/i, new ImageTexturePreviewLoader(manager, signal, "TGA", (reason) => {
    warn(errorMessage(reason, "Eine TGA-Modelltextur kann nicht angezeigt werden."));
  }, textureBudget));
  const gifLoader = new ImageTexturePreviewLoader(manager, signal, "GIF", (reason) => {
    warn(errorMessage(reason, "Eine GIF-Modelltextur kann nicht angezeigt werden."));
  }, textureBudget);
  manager.addHandler(/(?:\.gif(?:$|[?#])|^data:image\/gif[;,])/i, gifLoader);
  manager.addHandler(/\.(ase|aseprite)(?:$|[?#])/i, new ImageTexturePreviewLoader(manager, signal, "ASEPRITE", (reason) => {
    warn(errorMessage(reason, "Eine Aseprite-Modelltextur kann nicht angezeigt werden."));
  }, textureBudget));
  let markerEnded = false;
  let disposed = false;
  const dispose = () => {
    if (disposed) return;
    disposed = true;
    disposeObjects(objects, materials);
    for (const blob of blobs) URL.revokeObjectURL(blob);
  };
  const abort = () => manager.abort();
  signal.throwIfAborted();
  signal.addEventListener("abort", abort, { once: true });
  manager.onError = () => warn("Die Modell-Datei oder eine benötigte Ressource konnte nicht geladen werden.");
  const loaded = new Promise<void>((resolve) => { manager.onLoad = resolve; });
  manager.itemStart("local-model-preview");

  try {
    const base = `https://model.invalid/${encodeURIComponent(snapshot.snapshotId)}/`;
    const basePath = new URL(base).pathname;
    const encodedPath = (path: string) => path.split("/").map(encodeURIComponent).join("/");
    const entryUrl = base + encodedPath(snapshot.entryPath);
    let neutralTexture: string | null = null;
    manager.setURLModifier((url) => {
      if (url.startsWith("data:")) {
        if (/^data:image\/gif[;,]/i.test(url)) staticGifTextures = true;
        return modelTransportUrl(url, snapshot.snapshotId);
      }
      if (url.startsWith("blob:")) { blobs.add(url); return url; }
      const parsed = new URL(url);
      if (parsed.origin !== "https://model.invalid" || !parsed.pathname.startsWith(basePath) || parsed.search || parsed.hash) {
        throw new Error("Das Modell enthält einen externen oder ungültigen Ressourcenpfad.");
      }
      const decoded = decodeURIComponent(parsed.pathname.slice(basePath.length)).replace(/\\/g, "/").normalize("NFC");
      if (decoded.startsWith("/") || /(^|\/)[a-z][a-z\d+.-]*:/i.test(decoded) || decoded.includes("\0")) {
        throw new Error("Absolute oder externe Modell-Ressourcen werden nicht geladen.");
      }
      const parts: string[] = [];
      for (const part of decoded.split("/")) {
        if (!part || part === ".") continue;
        if (part === "..") {
          if (!parts.length) throw new Error("Modell-Ressource liegt außerhalb ihres Pakets.");
          parts.pop();
        } else parts.push(part);
      }
      const relative = parts.join("/");
      const resolution = resolveModelResource(relative, snapshot.format, snapshot.resources);
      if (!resolution.resource) {
        warn(`FBX-Textur fehlt im Paket: ${resolution.missingTexture}. Geometrie ohne diese Textur.`);
        if (/\.tga$/i.test(relative)) {
          // The TGA handler needs a TGA placeholder; ordinary image loaders use the PNG below.
          const tga = Uint8Array.from([0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 1, 0, 32, 40, 189, 185, 181, 255]);
          return modelTransportUrl(`data:image/x-tga;base64,${btoa(String.fromCharCode(...tga))}`, snapshot.snapshotId);
        }
        if (/\.gif$/i.test(relative)) {
          // The guarded GIF handler also needs a matching, complete container.
          return modelTransportUrl("data:image/gif;base64,R0lGODlhAQABAIAAAP///wAAACH5BAAAAAAALAAAAAABAAEAAAICRAEAOw==", snapshot.snapshotId);
        }
        if (!neutralTexture) {
          const canvas = document.createElement("canvas");
          canvas.width = canvas.height = 1;
          const context = canvas.getContext("2d");
          if (!context) throw new Error("Die neutrale Textur konnte nicht erstellt werden.");
          context.fillStyle = "#b5b9bd";
          context.fillRect(0, 0, 1, 1);
          neutralTexture = canvas.toDataURL("image/png");
        }
        return modelTransportUrl(neutralTexture, snapshot.snapshotId);
      }
      const { resource, substituted } = resolution;
      if (substituted) {
        const replacement = `${relative} → ${resource.relativePath}`;
        if (!replacements.includes(replacement)) replacements.push(replacement);
      }
      used.add(resource.relativePath);
      if (/\.gif$/i.test(resource.relativePath)) staticGifTextures = true;
      if (/\.(ase|aseprite)$/i.test(resource.relativePath)) staticAsepriteTextures = true;
      return modelTransportUrl(convertFileSrc(resource.path), snapshot.snapshotId);
    });
    let model: Object3D;
    let clips: AnimationClip[] = [];
    if (snapshot.format === "GLTF" || snapshot.format === "GLB") {
      const loader = new GLTFLoader(manager);
      loader.register((parser) => ({
        name: "local-preview-required-extensions",
        loadTexture: (textureIndex) => {
          const texture = parser.json.textures?.[textureIndex];
          const image = parser.json.images?.[texture?.source];
          if (image && (/^image\/gif(?:;|$)/i.test(image.mimeType ?? "") || /(?:\.gif(?:$|[?#])|^data:image\/gif[;,])/i.test(image.uri ?? ""))) {
            if (image.bufferView !== undefined) {
              const index = image.bufferView;
              const view = parser.json.bufferViews?.[index];
              const offset = view?.byteOffset ?? 0;
              const buffer = parser.json.buffers?.[view?.buffer];
              // GLTFLoader copies the view into a blob before our bounded GIF fetch starts.
              if (!Number.isSafeInteger(index) || index < 0 || !view
                || !Number.isSafeInteger(view.byteLength) || view.byteLength <= 0 || view.byteLength > maxGifBytes
                || !Number.isSafeInteger(offset) || offset < 0 || !Number.isSafeInteger(view.buffer) || view.buffer < 0
                || !Number.isSafeInteger(buffer?.byteLength) || offset + view.byteLength > buffer.byteLength) {
                throw new Error("GIF-Modelltextur: ungültige eingebettete Ressource oder mehr als 32 MiB.");
              }
            }
            staticGifTextures = true;
            return parser.loadTextureImage(textureIndex, texture.source, gifLoader);
          }
          return null;
        },
        beforeRoot: () => {
          if (parser.json.asset?.version !== "2.0") throw new Error("Die Vorschau unterstützt glTF 2.0.");
          const required: string[] = parser.json.extensionsRequired ?? [];
          const decoders = ["KHR_draco_mesh_compression", "KHR_texture_basisu", "EXT_meshopt_compression", "KHR_meshopt_compression"];
          const unsupported = required.filter((name) => decoders.includes(name) || (!parser.plugins[name] && !parser.extensions[name]));
          if (unsupported.length) throw new Error(`Diese glTF-Kodierung wird noch nicht unterstützt: ${unsupported.join(", ")}`);
          return null;
        },
      }));
      const gltf = await loader.loadAsync(entryUrl);
      objects.push(...gltf.scenes);
      model = gltf.scene;
      clips = gltf.animations;
    } else if (snapshot.format === "FBX") {
      model = await new FBXLoader(manager).loadAsync(entryUrl);
      objects.push(model);
      clips = model.animations;
    } else if (snapshot.format === "OBJ") {
      const text = (await new FileLoader(manager).loadAsync(entryUrl) as string).replace(/\\\r?\n/g, "");
      const directory = entryUrl.slice(0, entryUrl.lastIndexOf("/") + 1);
      const libraries = [...new Set(Array.from(text.matchAll(/^\s*mtllib\s+(.+)$/gm), (match) => match[1].trim()))];
      const creators: MTLLoader.MaterialCreator[] = [];
      const names = new Set<string>();
      for (const reference of libraries) {
        const url = directory + reference;
        const mtl = await new FileLoader(manager).loadAsync(url) as string;
        const creator = new MTLLoader(manager).parse(mtl, url.slice(0, url.lastIndexOf("/") + 1));
        for (const name of Object.keys(creator.materialsInfo)) {
          if (names.has(name)) throw new Error(`OBJ-Material ist in mehreren Bibliotheken definiert: ${name}`);
          names.add(name);
        }
        creator.preload();
        materials.push(...Object.values(creator.materials));
        creators.push(creator);
      }
      for (const match of text.matchAll(/^[ \t]*usemtl[ \t]+(.+)$/gm)) {
        const name = match[1].trim();
        if (name && !names.has(name)) throw new Error(`OBJ-Material ist nicht definiert: ${name}`);
      }
      const loader = new OBJLoader(manager);
      if (creators.length) {
        Object.assign(creators[0].materials, ...creators.slice(1).map((creator) => creator.materials));
        loader.setMaterials(creators[0]);
      }
      model = loader.parse(text);
      objects.push(model);
    } else throw new Error("Dieses Modellformat wird nicht unterstützt.");

    markerEnded = true;
    manager.itemEnd("local-model-preview");
    await loaded;
    signal.throwIfAborted();
    const stats = { meshes: 0, vertices: 0, triangles: 0, bones: 0 };
    const modelMaterials = new Set(materials);
    model.traverse((child) => {
      if (child instanceof Bone) stats.bones++;
      if (!(child instanceof Mesh)) return;
      stats.meshes++;
      const vertices = child.geometry.getAttribute("position")?.count ?? 0;
      stats.vertices += vertices;
      stats.triangles += Math.floor((child.geometry.index?.count ?? vertices) / 3);
      for (const material of Array.isArray(child.material) ? child.material : [child.material]) modelMaterials.add(material);
    });
    for (const material of modelMaterials) for (const value of Object.values(material)) {
      if (value instanceof Texture && !modelTextureAvailable(value)) {
        warn("Eine Modell-Textur ist nicht verfügbar oder ihr Bildformat wird nicht unterstützt.");
      }
    }
    const bounds = modelPreviewBounds(model);
    if (bounds.isEmpty() || ![...bounds.min.toArray(), ...bounds.max.toArray()].every(Number.isFinite)) {
      throw new Error("Das Modell enthält keine gültige sichtbare Geometrie.");
    }
    return { model, clips, stats, warnings, replacements, resourcePaths: [...used], staticGifTextures, staticAsepriteTextures, dispose };
  } catch (reason) {
    manager.abort();
    dispose();
    throw reason;
  } finally {
    if (!markerEnded) manager.itemEnd("local-model-preview");
    signal.removeEventListener("abort", abort);
  }
}

export type LoadedModel = Awaited<ReturnType<typeof loadModel>>;

export function modelScene(loaded: LoadedModel) {
  const scene = new Scene();
  scene.background = new Color("#202c3b");
  scene.add(loaded.model, new HemisphereLight(0xffffff, 0x7185a2, 2));
  let skeleton: SkeletonHelper | null = null;
  let joints: Points<BufferGeometry, PointsMaterial> | null = null;
  if (!loaded.stats.meshes && loaded.stats.bones) {
    skeleton = new SkeletonHelper(loaded.model);
    skeleton.setColors(new Color("#ffc46e"), new Color("#ffffff"));
    if (!skeleton.geometry.getAttribute("position").count || modelPreviewBounds(loaded.model).getSize(new Vector3()).lengthSq() === 0) {
      skeleton.dispose();
      throw new Error("Das Skelett enthält keine darstellbaren Gelenkabstände.");
    }
    skeleton.frustumCulled = false;
    const bones = skeleton.bones;
    const positions = new BufferAttribute(new Float32Array(bones.length * 3), 3);
    joints = new Points(new BufferGeometry().setAttribute("position", positions),
      new PointsMaterial({ color: "#ffffff", size: 3, sizeAttenuation: false, depthTest: false, depthWrite: false }));
    joints.frustumCulled = false;
    joints.renderOrder = 1;
    const joint = new Vector3();
    // Runs at render time, after the mixer update and world-matrix pass, so joints follow the pose.
    joints.onBeforeRender = () => {
      bones.forEach((bone, index) => positions.setXYZ(index, ...joint.setFromMatrixPosition(bone.matrixWorld).toArray()));
      positions.needsUpdate = true;
    };
    scene.add(skeleton, joints);
  }
  const light = new DirectionalLight(0xffffff, 3);
  light.position.set(3, 5, 4);
  scene.add(light);
  return { scene, dispose: () => { skeleton?.dispose(); joints?.geometry.dispose(); joints?.material.dispose(); } };
}

export function fitModelCamera(camera: PerspectiveCamera, model: Object3D) {
  const box = modelPreviewBounds(model);
  const center = box.getCenter(new Vector3());
  const radius = Math.max(box.getSize(new Vector3()).length() / 2, 0.001);
  // Fit the box corners, not its bounding sphere: compact or sprawling poses (skeletons) otherwise sit small in the frame.
  camera.position.copy(center).add(new Vector3(1, 0.7, 1.8).normalize());
  camera.lookAt(center);
  const toView = camera.quaternion.clone().invert();
  const points = Array.from({ length: 8 }, (_, corner) =>
    new Vector3(corner & 1 ? box.max.x : box.min.x, corner & 2 ? box.max.y : box.min.y, corner & 4 ? box.max.z : box.min.z));
  // Bare skeletons fit their joints; their box corners are far outside a crouching or reaching pose.
  const skeletonOnly = new Box3().setFromObject(model).isEmpty();
  if (skeletonOnly) {
    points.length = 0;
    model.traverse((child) => { if (child instanceof Bone) points.push(child.getWorldPosition(new Vector3())); });
  }
  // Joints get extra margin: limbs and root motion leave the fitted pose while the clip plays.
  const tanV = Math.tan(camera.fov * Math.PI / 360) * (skeletonOnly ? 0.7 : 0.9);
  const tanH = tanV * camera.aspect;
  let distance = radius * 0.01;
  for (const point of points) {
    const offset = point.sub(center).applyQuaternion(toView);
    distance = Math.max(distance, offset.z + Math.abs(offset.x) / tanH, offset.z + Math.abs(offset.y) / tanV);
  }
  camera.position.copy(center).add(new Vector3(1, 0.7, 1.8).normalize().multiplyScalar(distance));
  camera.near = Math.max(distance / 1000, 0.0001);
  camera.far = Math.max(distance * 100, 10);
  camera.updateProjectionMatrix();
  camera.lookAt(center);
  return { center, radius };
}
