import { useEffect, useRef, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { imageFit, prepareSpriteSequence, spriteFrameLimit, type ImageGroup } from "./imageWorkflows";
import { loadPreviewImage } from "./imagePreviewUrl.ts";
import type { ModelExportSelection } from "./ModelPreview";

type Frame = { index: number; path: string; width: number; height: number; image: HTMLImageElement; filename: string };

export default function SpriteSequencePreview({ group, visible, onReady }: {
  group: ImageGroup;
  visible: boolean;
  onReady: (selection: ModelExportSelection | null) => void;
}) {
  const viewport = useRef<HTMLDivElement>(null);
  const canvas = useRef<HTMLCanvasElement>(null);
  const [frames, setFrames] = useState<Frame[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [frame, setFrame] = useState(0);
  const [fps, setFps] = useState(8);
  const [playing, setPlaying] = useState(false);
  const [pixel, setPixel] = useState(true);
  const [transparency, setTransparency] = useState(true);
  const [zoom, setZoom] = useState(1);
  const [fitted, setFitted] = useState(true);
  const image = frames[frame];
  const validFps = Number.isInteger(fps) && fps >= 1 && fps <= 60;

  useEffect(() => {
    let active = true;
    const controller = new AbortController();
    const releases: Array<() => void> = [];
    setFrames([]);
    if (canvas.current) canvas.current.width = canvas.current.height = 1;
    setError(null);
    setPlaying(false);
    setFrame(0);
    onReady(null);
    void (async () => {
      try {
        if (group.errors.length || group.resources.errors.length) throw new Error([...group.errors, ...group.resources.errors].join(" · "));
        if (group.members.length > spriteFrameLimit) throw new Error("Die Sprite-Vorschau unterstützt maximal 256 Bilder pro Sequenz.");
        let retainedPixels = 0;
        const loaded: Frame[] = [];
        for (const member of group.members) {
          if (!active) return;
          const resource = group.resources.resources.find((item) => item.relativePath === member.resourcePath);
          if (!member.file.available || !resource || member.frameIndex === null) throw new Error(`Sprite-Bild ist nicht verfügbar: ${member.file.originalFilename}`);
          const preview = await loadPreviewImage(convertFileSrc(resource.path), member.file.originalFilename, controller.signal, 32 * 1024 * 1024 - retainedPixels);
          if (!active) { preview.release(); return; }
          releases.push(preview.release);
          const image = preview.image;
          retainedPixels += image.naturalWidth * image.naturalHeight;
          loaded.push({ index: member.frameIndex, path: member.resourcePath, width: image.naturalWidth, height: image.naturalHeight, image, filename: member.file.originalFilename });
        }
        if (!active) return;
        const ordered = prepareSpriteSequence(loaded);
        setFrames(ordered);
        onReady({ snapshotId: group.resources.snapshotId, fileId: group.resources.fileId, resourcePaths: ordered.map((item) => item.path) });
      } catch (reason) {
        for (const release of releases.splice(0)) release();
        if (active) { setError(reason instanceof Error ? reason.message : "Die Sprite-Sequenz kann nicht angezeigt werden."); onReady(null); }
      }
    })();
    return () => {
      active = false;
      controller.abort();
      onReady(null);
      for (const release of releases) release();
    };
  }, [group, onReady]);

  useEffect(() => {
    if (!image || !canvas.current) return;
    const context = canvas.current.getContext("2d");
    if (!context) { setError("Die Sprite-Vorschau ist in dieser Ansicht nicht verfügbar."); onReady(null); return; }
    canvas.current.width = image.width;
    canvas.current.height = image.height;
    context.imageSmoothingEnabled = !pixel;
    context.clearRect(0, 0, image.width, image.height);
    context.drawImage(image.image, 0, 0);
  }, [image, pixel, onReady]);

  useEffect(() => {
    if (!visible) setPlaying(false);
    if (!playing || !visible || !validFps || frames.length < 2) return;
    const timer = window.setInterval(() => setFrame((current) => (current + 1) % frames.length), 1000 / fps);
    return () => window.clearInterval(timer);
  }, [playing, visible, validFps, fps, frames.length]);

  useEffect(() => {
    const host = viewport.current;
    if (!host || !image || !fitted) return;
    const fit = () => { if (host.clientWidth > 0 && host.clientHeight > 0) setZoom(imageFit(image.width, image.height, host.clientWidth, host.clientHeight)); };
    fit();
    const resize = new ResizeObserver(fit);
    resize.observe(host);
    return () => resize.disconnect();
  }, [image?.width, image?.height, fitted]);

  function seek(position: number) { setPlaying(false); setFrame(Math.max(0, Math.min(frames.length - 1, position))); }

  return <>
    <div ref={viewport} className={`image-viewport ${transparency ? "transparency-background" : ""}`}>
      <canvas ref={canvas} aria-label={`Sprite-Vorschau: ${group.name}`} style={{ width: (image?.width ?? 1) * zoom, height: (image?.height ?? 1) * zoom, imageRendering: pixel ? "pixelated" : "auto" }} />
    </div>
    {!frames.length && !error && <p className="detail-hint" role="status">Sprite-Bilder werden geladen und geprüft …</p>}
    {error && <p className="inline-error" role="alert">{error}</p>}
    {group.members.some((member) => /\.gif$/i.test(member.file.originalFilename)) && <p className="detail-hint">GIF-Dateien verwenden in dieser Sequenz ihr erstes Bild.</p>}
    {group.members.some((member) => /\.(ase|aseprite)$/i.test(member.file.originalFilename)) && <p className="detail-hint">Ausgewählte Aseprite-Dateien verwenden hier ihr erstes vollständiges Bild.</p>}
    {image && <section className="sequence-controls" aria-label="Sprite-Animation">
      <div className="sequence-toolbar">
        <button className="button outlined" disabled={!visible || !validFps} onClick={() => setPlaying((current) => !current)}>{playing ? "Animation pausieren" : "Animation abspielen"}</button>
        <button className="button outlined" aria-label="Vorheriges Sprite-Bild" onClick={() => seek((frame + frames.length - 1) % frames.length)}>←</button>
        <button className="button outlined" aria-label="Nächstes Sprite-Bild" onClick={() => seek((frame + 1) % frames.length)}>→</button>
        <label>FPS<input aria-label="Sprite-FPS" type="number" min="1" max="60" step="1" value={fps} onChange={(event) => { setFps(Number(event.target.value)); setPlaying(false); }} /></label>
      </div>
      {!validFps && <p className="inline-error" role="alert">Wähle eine ganze Bildrate zwischen 1 und 60 FPS.</p>}
      <label className="sequence-position">Animationsposition<input aria-label="Sprite-Animationsposition" type="range" min="0" max={frames.length - 1} step="1" value={frame} onChange={(event) => seek(Number(event.target.value))} /></label>
      <p className="detail-hint" role="status">Bild {frame + 1} / {frames.length} · Frame {image.index} · {image.width} × {image.height} Pixel · {image.filename}</p>
      <div className="visual-controls">
        <label>Zoom · {Math.round(zoom * 100)} %<input aria-label="Sprite-Zoom" type="range" min="0.01" max="8" step="0.01" value={zoom} onChange={(event) => { setFitted(false); setZoom(Number(event.target.value)); }} /></label>
        <div className="visual-buttons"><button className="button outlined" onClick={() => setFitted(true)}>Einpassen</button><button className="button outlined" onClick={() => { setFitted(false); setZoom(1); }}>100 %</button></div>
        <div className="sequence-options">
          <label className="checkbox-label"><input type="checkbox" checked={pixel} onChange={(event) => setPixel(event.target.checked)} />Pixelansicht</label>
          <label className="checkbox-label"><input type="checkbox" checked={transparency} onChange={(event) => setTransparency(event.target.checked)} />Transparenzraster</label>
        </div>
      </div>
    </section>}
  </>;
}
