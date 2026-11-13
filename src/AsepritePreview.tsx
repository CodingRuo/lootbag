import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { fileSrc } from "./fileSrc.ts";
import { imageFit } from "./imageWorkflows.ts";
import { loadAsepriteAnimation } from "./imagePreviewUrl.ts";
import { errorMessage } from "./errorMessage.ts";

type Animation = Awaited<ReturnType<typeof loadAsepriteAnimation>>;
export default function AsepritePreview({ file }: { file: { id: number; originalFilename: string } }) {
  const viewport = useRef<HTMLDivElement>(null), canvas = useRef<HTMLCanvasElement>(null);
  const [animation, setAnimation] = useState<Animation | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [frame, setFrame] = useState(0), [playing, setPlaying] = useState(false);
  const [zoom, setZoom] = useState(1), [pixel, setPixel] = useState(true);
  const current = animation?.frames[frame];
  const count = animation?.frames.length ?? 0;

  function fit() {
    if (animation) setZoom(imageFit(animation.width, animation.height, viewport.current?.clientWidth ?? 240, viewport.current?.clientHeight ?? 240));
  }
  function seek(index: number) { setPlaying(false); setFrame(index); }

  useEffect(() => {
    let active = true;
    const controller = new AbortController(); let release = () => {};
    setAnimation(null); setError(null); setFrame(0); setPlaying(false);
    if (canvas.current) canvas.current.width = canvas.current.height = 1;
    void invoke<string>("resolve_preview", { fileId: file.id }).then(async (path) => {
      const preview = await loadAsepriteAnimation(fileSrc(path), controller.signal);
      if (!active) { preview.release(); return; }
      release = preview.release; setAnimation(preview);
      setZoom(imageFit(preview.width, preview.height, viewport.current?.clientWidth ?? 240, viewport.current?.clientHeight ?? 240));
    }).catch((reason: unknown) => {
      if (active) setError(errorMessage(reason, "Die Aseprite-Datei ist nicht verfügbar."));
    });
    return () => { active = false; controller.abort(); release(); };
  }, [file.id, file.originalFilename]);

  useEffect(() => {
    if (!current || !animation || !canvas.current) return;
    const context = canvas.current.getContext("2d");
    if (!context) { setError("Die Aseprite-Bildvorschau ist in dieser Ansicht nicht verfügbar."); setPlaying(false); return; }
    canvas.current.width = animation.width; canvas.current.height = animation.height;
    context.putImageData(new ImageData(new Uint8ClampedArray(current.buffer), animation.width, animation.height), 0, 0);
  }, [animation, current]);

  useEffect(() => {
    if (!playing || !current || count < 2) return;
    const timer = window.setTimeout(() => setFrame((previous) => (previous + 1) % count), current.durationMs);
    return () => window.clearTimeout(timer);
  }, [playing, current, count]);

  return <>
    <div ref={viewport} className="image-viewport transparency-background">
      <canvas ref={canvas} aria-label={`Aseprite-Frame: ${file.originalFilename}`} style={{ width: (animation?.width ?? 1) * zoom, height: (animation?.height ?? 1) * zoom, imageRendering: pixel ? "pixelated" : "auto" }} />
    </div>
    {!animation && !error && <p className="detail-hint" role="status">Aseprite-Layer und Frames werden geprüft …</p>}
    {error && <p className="inline-error" role="alert">{error}</p>}
    {animation && current && <>
      <div className="visual-buttons">
        <button className="button outlined" disabled={count < 2 || Boolean(error)} onClick={() => setPlaying((value) => !value)}>{playing ? "Pausieren" : "Animation abspielen"}</button>
        <button className="button outlined" onClick={() => seek((frame + 1) % count)}>Nächstes Bild</button>
        <button className="button outlined" onClick={() => seek(0)}>Erstes Bild</button>
      </div>
      <label>Animationsposition<input aria-label="Aseprite-Frame" type="range" min="0" max={count - 1} step="1" value={frame} onChange={(event) => seek(Number(event.target.value))} /></label>
      <p className="detail-hint" role="status">Bild {frame + 1} / {count} · {current.durationMs} ms · {animation.width} × {animation.height} Pixel</p>
      <div className="visual-controls">
        <label>Zoom · {Math.round(zoom * 100)} %<input aria-label="Aseprite-Zoom" type="range" min="0.01" max="8" step="0.01" value={zoom} onChange={(event) => setZoom(Number(event.target.value))} /></label>
        <div className="visual-buttons"><button className="button outlined" onClick={fit}>Einpassen</button><button className="button outlined" onClick={() => setZoom(1)}>100 %</button></div>
        <label className="checkbox-label"><input type="checkbox" checked={pixel} onChange={(event) => setPixel(event.target.checked)} />Pixelansicht</label>
      </div>
    </>}
  </>;
}
