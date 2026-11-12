import { useEffect, useRef, useState } from "react";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { imageFit } from "./imageWorkflows.ts";
import { loadGifAnimation, loadPreviewImage } from "./imagePreviewUrl.ts";
import { errorMessage } from "./errorMessage.ts";

export default function GifPreview({ file }: { file: { id: number; originalFilename: string } }) {
  // ponytail: Native image playback has no seek/current-frame pause; use an indexed decoder if GIF editing is added.
  const viewport = useRef<HTMLDivElement>(null);
  const [image, setImage] = useState<HTMLImageElement | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [animated, setAnimated] = useState(true);
  const [frames, setFrames] = useState<number | null>(null);
  const [zoom, setZoom] = useState(1);
  const [pixel, setPixel] = useState(true);

  function fit() {
    if (image) setZoom(imageFit(image.naturalWidth, image.naturalHeight, viewport.current?.clientWidth ?? 240, viewport.current?.clientHeight ?? 240));
  }

  useEffect(() => {
    let active = true;
    const controller = new AbortController();
    let release = () => {};
    setImage(null); setError(null); setFrames(null);
    void invoke<string>("resolve_preview", { fileId: file.id }).then(async (path) => {
      const source = convertFileSrc(path);
      const preview = animated ? await loadGifAnimation(source, controller.signal)
        : await loadPreviewImage(source, file.originalFilename, controller.signal);
      if (!active) { preview.release(); return; }
      release = preview.release;
      if ("frames" in preview && typeof preview.frames === "number") setFrames(preview.frames);
      setImage(preview.image);
      setZoom(imageFit(preview.image.naturalWidth, preview.image.naturalHeight, viewport.current?.clientWidth ?? 240, viewport.current?.clientHeight ?? 240));
    }).catch((reason: unknown) => {
      if (active) setError(errorMessage(reason, "Die GIF-Datei ist nicht verfügbar."));
    });
    return () => { active = false; controller.abort(); release(); };
  }, [file.id, file.originalFilename, animated]);

  useEffect(() => {
    if (!image || !viewport.current) return;
    // Reuse the guarded decoded image so displaying it does not start another GIF decoder.
    image.alt = `${animated ? "GIF-Animation" : "Erstes GIF-Bild"}: ${file.originalFilename}`;
    viewport.current.appendChild(image);
    return () => image.remove();
  }, [image, animated, file.originalFilename]);

  useEffect(() => {
    if (!image) return;
    image.style.width = `${image.naturalWidth * zoom}px`;
    image.style.height = `${image.naturalHeight * zoom}px`;
    image.style.imageRendering = pixel ? "pixelated" : "auto";
  }, [image, zoom, pixel]);

  return <>
    <div ref={viewport} className="image-viewport transparency-background" />
    {!image && !error && <p className="detail-hint" role="status">GIF wird geladen und geprüft …</p>}
    {error && <p className="inline-error" role="alert">{error}</p>}
    <div className="visual-buttons">
      <button className="button outlined" aria-pressed={animated} onClick={() => setAnimated(true)}>Animation anzeigen</button>
      <button className="button outlined" aria-pressed={!animated} onClick={() => setAnimated(false)}>Erstes Bild</button>
    </div>
    {image && <>
      <p className="detail-hint">{image.naturalWidth} × {image.naturalHeight} Pixel{frames !== null ? ` · ${frames} ${frames === 1 ? "Bild" : "Bilder"}` : ""} · Transparenz auf Schachbrett</p>
      <div className="visual-controls">
        <label>Zoom · {Math.round(zoom * 100)} %<input aria-label="GIF-Zoom" type="range" min="0.01" max="8" step="0.01" value={zoom} onChange={(event) => setZoom(Number(event.target.value))} /></label>
        <div className="visual-buttons"><button className="button outlined" onClick={fit}>Einpassen</button><button className="button outlined" onClick={() => setZoom(1)}>100 %</button></div>
        <label className="checkbox-label"><input type="checkbox" checked={pixel} onChange={(event) => setPixel(event.target.checked)} />Pixelansicht</label>
      </div>
    </>}
  </>;
}
