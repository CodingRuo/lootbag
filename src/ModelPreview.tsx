import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { PerspectiveCamera, WebGLRenderer } from "three";
import { OrbitControls } from "three/addons/controls/OrbitControls.js";
import { fitModelCamera, loadModel, modelScene, type LoadedModel, type ModelResources } from "./loadModel";
import { ModelAnimation } from "./modelAnimation";
import "./modelAnimation.css";
import { errorMessage } from "./errorMessage.ts";

export type ModelExportSelection = { snapshotId: string; fileId: number; resourcePaths: string[] };

type Playback = { animation: ModelAnimation; start: () => void; stop: () => void; render: () => void; sync: () => void };

export default function ModelPreview({ file, onReady, visible = true }: {
  file: { id: number; originalFilename: string };
  onReady: (selection: ModelExportSelection | null) => void;
  visible?: boolean;
}) {
  const viewport = useRef<HTMLDivElement>(null);
  const controls = useRef<OrbitControls | null>(null);
  const fit = useRef<(() => void) | null>(null);
  const playback = useRef<Playback | null>(null);
  const isVisible = useRef(visible);
  const [loading, setLoading] = useState(true);
  const [errors, setErrors] = useState<string[]>([]);
  const [textureReplacements, setTextureReplacements] = useState<string[]>([]);
  const [gifTextures, setGifTextures] = useState(false);
  const [asepriteTextures, setAsepriteTextures] = useState(false);
  const [stats, setStats] = useState<{ meshes: number; vertices: number; triangles: number; bones: number } | null>(null);
  const [clips, setClips] = useState<{ name: string; duration: number }[]>([]);
  const [animationState, setAnimationState] = useState<ModelAnimation["state"] | null>(null);
  const [animationError, setAnimationError] = useState("");

  useEffect(() => {
    isVisible.current = visible;
    if (!visible) { playback.current?.stop(); playback.current?.sync(); }
  }, [visible]);

  function changeAnimation(change: (animation: ModelAnimation) => void) {
    const player = playback.current;
    if (!player) return;
    change(player.animation);
    if (player.animation.state.playing) player.start();
    else player.stop();
    player.render();
    player.sync();
  }

  useEffect(() => {
    let active = true;
    let renderer: WebGLRenderer | null = null;
    let orbit: OrbitControls | null = null;
    let resize: ResizeObserver | null = null;
    let player: Playback | null = null;
    let frame = 0;
    const controller = new AbortController();
    let loadedModel: LoadedModel | null = null;
    let modelView: ReturnType<typeof modelScene> | null = null;
    setLoading(true);
    setErrors([]);
    setTextureReplacements([]);
    setGifTextures(false);
    setAsepriteTextures(false);
    setStats(null);
    setClips([]);
    setAnimationState(null);
    setAnimationError("");
    onReady(null);
    async function load() {
      try {
        const snapshot = await invoke<ModelResources>("model_resources", { fileId: file.id });
        if (!active) return;
        loadedModel = await loadModel(snapshot, controller.signal);
        if (!active) { loadedModel.dispose(); return; }
        const { model, clips: modelClips, stats: totals, warnings, replacements, resourcePaths } = loadedModel;
        setErrors(warnings);
        setTextureReplacements(replacements);
        setGifTextures(loadedModel.staticGifTextures);
        setAsepriteTextures(loadedModel.staticAsepriteTextures);
        const host = viewport.current;
        if (!host) return;
        renderer = new WebGLRenderer({ antialias: true });
        renderer.setPixelRatio(Math.min(window.devicePixelRatio, 2));
        renderer.domElement.tabIndex = 0;
        renderer.domElement.setAttribute("aria-label", `3D-Modellvorschau: ${file.originalFilename}`);
        host.append(renderer.domElement);
        modelView = modelScene(loadedModel);
        const { scene } = modelView;
        const camera = new PerspectiveCamera(45, 1, 0.01, 1000);
        orbit = new OrbitControls(camera, renderer.domElement);
        controls.current = orbit;
        orbit.listenToKeyEvents(renderer.domElement);
        const render = () => { if (active && renderer) renderer.render(scene, camera); };
        orbit.addEventListener("change", render);
        if (modelClips.length) {
          try {
            const animation = new ModelAnimation(model, modelClips);
            const sync = () => { if (active) setAnimationState(animation.state); };
            const stop = () => { cancelAnimationFrame(frame); frame = 0; animation.pause(); };
            let previous = 0;
            let lastSync = 0;
            const tick = (now: number) => {
              frame = 0;
              if (!active || !isVisible.current || document.hidden) { stop(); sync(); return; }
              animation.update(Math.min((now - previous) / 1000, 0.1));
              previous = now;
              render();
              if (now - lastSync >= 100 || !animation.state.playing) { sync(); lastSync = now; }
              if (animation.state.playing) frame = requestAnimationFrame(tick);
            };
            const start = () => {
              if (frame || !active || !isVisible.current || document.hidden) return;
              previous = performance.now();
              frame = requestAnimationFrame(tick);
            };
            player = { animation, start, stop, render, sync };
            playback.current = player;
            setClips(modelClips.map((clip, index) => ({ name: clip.name || `Clip ${index + 1}`, duration: clip.duration })));
            sync();
          } catch (reason: unknown) {
            setAnimationError(errorMessage(reason, "Diese Animation wird nicht unterstützt."));
          }
        }
        fit.current = () => {
          if (!orbit) return;
          const { center, radius } = fitModelCamera(camera, model);
          orbit.target.copy(center);
          orbit.minDistance = radius * 0.05;
          orbit.maxDistance = radius * 100;
          orbit.update();
          orbit.saveState();
          render();
        };
        function sizeRenderer() {
          if (!renderer) return;
          const width = Math.max(host!.clientWidth, 1);
          const height = Math.max(host!.clientHeight, 1);
          renderer.setSize(width, height, false);
          camera.aspect = width / height;
          camera.updateProjectionMatrix();
          render();
        }
        sizeRenderer();
        fit.current();
        resize = new ResizeObserver(sizeRenderer);
        resize.observe(host);
        setStats(totals);
        if (!warnings.length && !replacements.length) onReady({ snapshotId: snapshot.snapshotId, fileId: snapshot.fileId, resourcePaths });
      } catch (reason: unknown) {
        if (active) setErrors((previous) => [...previous, errorMessage(reason, "Die Modellvorschau ist nicht verfügbar.")]);
      } finally {
        if (active) setLoading(false);
        else { modelView?.dispose(); loadedModel?.dispose(); }
      }
    }

    const pauseHiddenDocument = () => { if (document.hidden) { player?.stop(); player?.sync(); } };
    document.addEventListener("visibilitychange", pauseHiddenDocument);
    void load();
    return () => {
      active = false;
      player?.stop();
      player?.animation.dispose();
      if (playback.current === player) playback.current = null;
      document.removeEventListener("visibilitychange", pauseHiddenDocument);
      onReady(null);
      controller.abort();
      resize?.disconnect();
      orbit?.dispose();
      if (controls.current === orbit) controls.current = null;
      fit.current = null;
      modelView?.dispose();
      loadedModel?.dispose();
      renderer?.dispose();
      renderer?.forceContextLoss();
      renderer?.domElement.remove();
    };
  }, [file.id, file.originalFilename, onReady]);

  return <>
    <div ref={viewport} className="model-viewport" />
    {loading && <p className="detail-hint" role="status">Modell und Ressourcen werden geladen …</p>}
    {gifTextures && <p className="detail-hint">GIF-Modelltexturen zeigen das erste Bild der Animation.</p>}
    {asepriteTextures && <p className="detail-hint">Aseprite-Modelltexturen zeigen das erste vollständige Bild der Timeline.</p>}
    {errors.length > 0 && <div className="inline-error" role="alert"><p>{stats ? "Modellvorschau mit fehlenden Ressourcen. Vollständiger Export nicht verfügbar." : "Die Modellvorschau ist unvollständig oder nicht verfügbar."}</p>
      <ul>{errors.slice(0, 8).map((error) => <li key={error}>{error}</li>)}</ul>{errors.length > 8 && <p>Weitere {errors.length - 8} Ressourcenfehler.</p>}</div>}
    {textureReplacements.length > 0 && <div className="inline-error" role="status"><p>Texturersatz nur für die Vorschau:</p>
      <ul>{textureReplacements.slice(0, 8).map((replacement) => <li key={replacement}>{replacement}</li>)}</ul>
      {textureReplacements.length > 8 && <p>Weitere {textureReplacements.length - 8} Ersatztexturen.</p>}
      <p>Der vollständige Export ist gesperrt, weil die Originaldatei weiterhin PSD referenziert. Die Quelldateien bleiben unverändert.</p>
    </div>}
    {stats && <>
      <p className="detail-hint">{stats.meshes} Meshes · {stats.vertices.toLocaleString("de-DE")} Vertices · {stats.triangles.toLocaleString("de-DE")} Dreiecke{stats.bones > 0 && ` · ${stats.bones} Bones`}</p>
      <div className="visual-buttons camera-controls">
        <button className="button outlined" aria-label="Modell nach links drehen" onClick={() => controls.current?.rotateLeft(Math.PI / 8)}>←</button>
        <button className="button outlined" aria-label="Modell nach rechts drehen" onClick={() => controls.current?.rotateLeft(-Math.PI / 8)}>→</button>
        <button className="button outlined" aria-label="Modell nach oben drehen" onClick={() => controls.current?.rotateUp(Math.PI / 8)}>↑</button>
        <button className="button outlined" aria-label="Modell nach unten drehen" onClick={() => controls.current?.rotateUp(-Math.PI / 8)}>↓</button>
        <button className="button outlined" aria-label="Modell vergrößern" onClick={() => controls.current?.dollyIn(1 / 1.2)}>+</button>
        <button className="button outlined" aria-label="Modell verkleinern" onClick={() => controls.current?.dollyOut(1 / 1.2)}>−</button>
        <button className="button outlined" onClick={() => fit.current?.()}>Einpassen</button>
        <button className="button outlined" onClick={() => controls.current?.reset()}>Zurücksetzen</button>
      </div>
      <p className="detail-hint">Ziehen dreht · Mausrad zoomt · Pfeiltasten verschieben, mit Shift drehen sie.</p>
      {animationError && <p className="inline-error" role="alert">Animationsvorschau nicht verfügbar: {animationError}</p>}
      {!animationState && !animationError && <p className="detail-hint">Keine eingebetteten Animations-Clips. Dieses Modell ist statisch.</p>}
      {animationState && <section className="model-animation" aria-label="3D-Animation">
        <div className="model-animation-fields">
          <label>Animations-Clip
            <select value={animationState.selected} onChange={(event) => changeAnimation((animation) => animation.select(Number(event.target.value)))}>
              {clips.map((clip, index) => <option key={index} value={index}>{clip.name} · {clip.duration.toFixed(2)} s</option>)}
            </select>
          </label>
          <label>Geschwindigkeit
            <select value={animationState.speed} onChange={(event) => changeAnimation((animation) => animation.setSpeed(Number(event.target.value)))}>
              {[0.25, 0.5, 1, 1.5, 2].map((speed) => <option key={speed} value={speed}>{speed}×</option>)}
            </select>
          </label>
        </div>
        <div className="model-animation-actions">
          <button className="button outlined" disabled={!visible} onClick={() => changeAnimation((animation) => animation.state.playing ? animation.pause() : animation.play())}>
            {animationState.playing ? "Animation pausieren" : "Animation abspielen"}
          </button>
          <label><input type="checkbox" checked={animationState.loop} onChange={(event) => changeAnimation((animation) => animation.setLoop(event.target.checked))} /> Loop</label>
          <output aria-label="Animations-Zeit">{animationState.time.toFixed(2)} / {animationState.duration.toFixed(2)} s</output>
        </div>
        <label className="model-animation-timeline">Animationsposition
          <input type="range" min="0" max={animationState.duration} step="0.01" value={animationState.time}
            onChange={(event) => changeAnimation((animation) => animation.seek(Number(event.target.value)))} />
        </label>
        <p className="detail-hint">Clips laufen auf dem mitgelieferten Modell oder Skelett. Retargeting auf andere Modelle wird nicht geprüft.</p>
      </section>}
    </>}
  </>;
}
