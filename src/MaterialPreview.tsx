import { useEffect, useRef, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import {
  Color, DirectionalLight, HemisphereLight, Mesh, MeshStandardMaterial,
  PerspectiveCamera, PlaneGeometry, RepeatWrapping, Scene, SphereGeometry,
  SRGBColorSpace, Texture, WebGLRenderer,
} from "three";
import { OrbitControls } from "three/addons/controls/OrbitControls.js";
import { defaultMaterialChoices, materialRoles, materialSelection, type ImageGroup, type MaterialRole } from "./imageWorkflows";
import { loadPreviewImage } from "./imagePreviewUrl.ts";
import type { ModelExportSelection } from "./ModelPreview";
import { errorMessage } from "./errorMessage.ts";

const labels: Record<MaterialRole, string> = {
  baseColor: "Base Color", normal: "Normal", roughness: "Roughness", metallic: "Metallic", ao: "Ambient Occlusion", height: "Height",
};
type Runtime = {
  mesh: Mesh<PlaneGeometry | SphereGeometry, MeshStandardMaterial>;
  textures: Map<MaterialRole, Texture>;
  orbit: OrbitControls;
  render: () => void;
};

export default function MaterialPreview({ group, onReady, visible }: {
  group: ImageGroup;
  onReady: (selection: ModelExportSelection | null) => void;
  visible: boolean;
}) {
  const viewport = useRef<HTMLDivElement>(null);
  const runtime = useRef<Runtime | null>(null);
  const [choices, setChoices] = useState(() => defaultMaterialChoices(group));
  const [shape, setShape] = useState<"sphere" | "plane">("sphere");
  const [repeat, setRepeat] = useState(1);
  const [normalConvention, setNormalConvention] = useState<"opengl" | "directx">("opengl");
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const unresolved = materialRoles.some((role) => group.members.filter((member) => member.role === role).length > 1 && !choices[role]);
  const parameters = useRef({ repeat, normalConvention, shape });
  parameters.current = { repeat, normalConvention, shape };

  function updatePreview() {
    const preview = runtime.current;
    if (!preview) return;
    for (const texture of preview.textures.values()) texture.repeat.set(parameters.current.repeat, parameters.current.repeat);
    preview.mesh.material.normalScale.set(1, parameters.current.normalConvention === "directx" ? -1 : 1);
    const sphere = preview.mesh.geometry instanceof SphereGeometry;
    if (sphere !== (parameters.current.shape === "sphere")) {
      preview.mesh.geometry.dispose();
      preview.mesh.geometry = parameters.current.shape === "sphere" ? new SphereGeometry(1, 64, 48) : new PlaneGeometry(2.4, 2.4, 64, 64);
      preview.mesh.geometry.setAttribute("uv1", preview.mesh.geometry.getAttribute("uv").clone());
    }
    preview.render();
  }

  useEffect(updatePreview, [repeat, normalConvention, shape]);
  useEffect(() => { if (runtime.current) runtime.current.orbit.enabled = visible; }, [visible]);

  useEffect(() => {
    let active = true;
    let renderer: WebGLRenderer | null = null;
    let orbit: OrbitControls | null = null;
    let resize: ResizeObserver | null = null;
    let mesh: Runtime["mesh"] | null = null;
    const textures = new Map<MaterialRole, Texture>();
    const controller = new AbortController();
    const releases: Array<() => void> = [];
    setLoading(true);
    setError(null);
    onReady(null);
    void (async () => {
      try {
        const paths = materialSelection(group, choices);
        const host = viewport.current;
        if (!host) return;
        renderer = new WebGLRenderer({ antialias: true });
        renderer.setPixelRatio(Math.min(window.devicePixelRatio, 2));
        renderer.outputColorSpace = SRGBColorSpace;
        renderer.domElement.tabIndex = 0;
        renderer.domElement.setAttribute("aria-label", `Materialvorschau: ${group.name}`);
        host.appendChild(renderer.domElement);
        const scene = new Scene();
        scene.background = new Color("#202c3b");
        scene.add(new HemisphereLight(0xffffff, 0x53647a, 2));
        const light = new DirectionalLight(0xffffff, 3);
        light.position.set(3, 4, 4);
        scene.add(light);
        const camera = new PerspectiveCamera(40, 1, 0.1, 100);
        camera.position.set(0, 0, 3.7);
        const material = new MeshStandardMaterial({ roughness: 1, metalness: 0 });
        mesh = new Mesh(new SphereGeometry(1, 64, 48), material);
        mesh.geometry.setAttribute("uv1", mesh.geometry.getAttribute("uv").clone());
        scene.add(mesh);
        let retainedPixels = 0;
        for (const path of paths) {
          controller.signal.throwIfAborted();
          const member = group.members.find((item) => item.resourcePath === path)!;
          const resource = group.resources.resources.find((item) => item.relativePath === path)!;
          const preview = await loadPreviewImage(convertFileSrc(resource.path), member.file.originalFilename, controller.signal, 32 * 1024 * 1024 - retainedPixels);
          if (!active) { preview.release(); return; }
          releases.push(preview.release);
          retainedPixels += preview.image.naturalWidth * preview.image.naturalHeight;
          const texture = new Texture(preview.image);
          texture.needsUpdate = true;
          texture.wrapS = texture.wrapT = RepeatWrapping;
          if (member.role === "baseColor") texture.colorSpace = SRGBColorSpace;
          textures.set(member.role!, texture);
        }
        material.map = textures.get("baseColor") ?? null;
        material.normalMap = textures.get("normal") ?? null;
        material.roughnessMap = textures.get("roughness") ?? null;
        material.metalnessMap = textures.get("metallic") ?? null;
        material.metalness = material.metalnessMap ? 1 : 0;
        material.aoMap = textures.get("ao") ?? null;
        if (material.aoMap) material.aoMap.channel = 1;
        material.displacementMap = textures.get("height") ?? null;
        material.displacementScale = material.displacementMap ? 0.1 : 0;
        material.displacementBias = material.displacementMap ? -0.05 : 0;
        material.needsUpdate = true;
        const draw = () => { if (active && renderer) renderer.render(scene, camera); };
        orbit = new OrbitControls(camera, renderer.domElement);
        orbit.listenToKeyEvents(renderer.domElement);
        orbit.enablePan = false;
        orbit.minDistance = 1.8;
        orbit.maxDistance = 8;
        orbit.saveState();
        orbit.addEventListener("change", draw);
        const resizePreview = () => {
          if (!renderer || host.clientWidth < 1 || host.clientHeight < 1) return;
          renderer.setSize(host.clientWidth, host.clientHeight);
          camera.aspect = host.clientWidth / host.clientHeight;
          camera.updateProjectionMatrix();
          draw();
        };
        runtime.current = { mesh, textures, orbit, render: draw };
        updatePreview();
        resize = new ResizeObserver(resizePreview);
        resize.observe(host);
        resizePreview();
        setLoading(false);
        onReady({ snapshotId: group.resources.snapshotId, fileId: group.resources.fileId, resourcePaths: paths });
      } catch (reason) {
        if (!active) return;
        for (const texture of textures.values()) texture.dispose();
        textures.clear();
        for (const release of releases.splice(0)) release();
        setLoading(false);
        setError(errorMessage(reason, "Eine Material-Map kann nicht angezeigt werden."));
        onReady(null);
      }
    })();
    return () => {
      active = false;
      controller.abort();
      onReady(null);
      runtime.current = null;
      resize?.disconnect();
      orbit?.dispose();
      mesh?.geometry.dispose();
      mesh?.material.dispose();
      for (const texture of textures.values()) texture.dispose();
      for (const release of releases) release();
      renderer?.dispose();
      renderer?.forceContextLoss();
      renderer?.domElement.remove();
    };
  }, [group, choices, onReady]);

  return <>
    <div ref={viewport} className="model-viewport material-viewport" />
    {loading && <p className="detail-hint" role="status">Material-Maps werden geladen …</p>}
    {error && <p className="inline-error" role="alert">{error}</p>}
    {group.resources.errors.length > 0 && <p className="detail-hint" role="status">Nicht verfügbare Material-Maps: {group.resources.errors.join(" · ")}</p>}
    {!loading && !error && <div className="visual-buttons camera-controls">
      <button className="button outlined" aria-label="Material nach links drehen" onClick={() => runtime.current?.orbit.rotateLeft(Math.PI / 8)}>←</button>
      <button className="button outlined" aria-label="Material nach rechts drehen" onClick={() => runtime.current?.orbit.rotateLeft(-Math.PI / 8)}>→</button>
      <button className="button outlined" aria-label="Material nach oben drehen" onClick={() => runtime.current?.orbit.rotateUp(Math.PI / 8)}>↑</button>
      <button className="button outlined" aria-label="Material nach unten drehen" onClick={() => runtime.current?.orbit.rotateUp(-Math.PI / 8)}>↓</button>
      <button className="button outlined" aria-label="Material vergrößern" onClick={() => runtime.current?.orbit.dollyIn(1 / 1.2)}>+</button>
      <button className="button outlined" aria-label="Material verkleinern" onClick={() => runtime.current?.orbit.dollyOut(1 / 1.2)}>−</button>
      <button className="button outlined" aria-label="Materialkamera zurücksetzen" onClick={() => runtime.current?.orbit.reset()}>Zurücksetzen</button>
    </div>}
    <div className="material-controls">
      <label>Vorschauform<select aria-label="Material-Vorschauform" value={shape} onChange={(event) => setShape(event.target.value as typeof shape)}>
        <option value="sphere">Kugel</option><option value="plane">Fläche</option>
      </select></label>
      <label>Kachelung · {repeat}×<input aria-label="Material-Kachelung" type="range" min="1" max="8" step="1" value={repeat} onChange={(event) => setRepeat(Number(event.target.value))} /></label>
      <label>Normal-Konvention<select aria-label="Normal-Konvention" value={normalConvention} onChange={(event) => setNormalConvention(event.target.value as typeof normalConvention)}>
        <option value="opengl">OpenGL · +Y</option><option value="directx">DirectX · −Y</option>
      </select></label>
    </div>
    <details className="material-maps" open={unresolved}>
      <summary>Material-Maps · {group.members.length} Dateien{unresolved && " · Auswahl erforderlich"}</summary>
      <div className="material-map-fields">
        {materialRoles.map((role) => {
          const members = group.members.filter((member) => member.role === role);
          return <label key={role}>{labels[role]}
            <select aria-label={`${labels[role]}-Map`} value={choices[role] ?? ""} disabled={members.length === 0}
              onChange={(event) => setChoices((previous) => ({ ...previous, [role]: event.target.value }))}>
              <option value="">{members.length === 0 ? "Nicht vorhanden" : "Map auswählen"}</option>
              {members.map((member) => <option key={member.file.id} value={member.resourcePath}>{member.file.originalFilename}{!member.file.available && " · nicht verfügbar"}</option>)}
              {members.length > 0 && <option value="none">Ohne diese Map</option>}
            </select>
          </label>;
        })}
      </div>
      <p className="detail-hint">Fehlende Maps verwenden neutrale Werte.</p>
    </details>
    <p className="detail-hint">Ziehen oder Shift + Pfeiltasten drehen · Mausrad zoomt.</p>
  </>;
}
