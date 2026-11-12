import { useEffect, useRef, useState } from "react";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { imageFit } from "./imageWorkflows.ts";
import { loadPreviewImage } from "./imagePreviewUrl.ts";
import { blendEntryResource } from "./modelResources.ts";
import { modelTransportUrl, type ModelResources } from "./loadModel.ts";
import { errorMessage } from "./errorMessage.ts";

export default function BlendPreview({ file }: { file: { id: number; originalFilename: string } }) {
  const viewport = useRef<HTMLDivElement>(null);
  const [image, setImage] = useState<HTMLImageElement | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [zoom, setZoom] = useState(1);
  const [pixel, setPixel] = useState(true);

  function fit() {
    if (image) setZoom(imageFit(image.naturalWidth, image.naturalHeight, viewport.current?.clientWidth ?? 240, viewport.current?.clientHeight ?? 240));
  }

  useEffect(() => {
    let active = true;
    const controller = new AbortController();
    let release = () => {};
    setImage(null); setError(null);
    void invoke<ModelResources>("model_resources", { fileId: file.id }).then(async (snapshot) => {
      controller.signal.throwIfAborted();
      const entry = blendEntryResource(snapshot);
      const preview = await loadPreviewImage(modelTransportUrl(convertFileSrc(entry.path), snapshot.snapshotId), entry.relativePath, controller.signal, 4 * 1024 * 1024);
      if (!active) { preview.release(); return; }
      release = preview.release;
      setImage(preview.image);
      setZoom(imageFit(preview.image.naturalWidth, preview.image.naturalHeight, viewport.current?.clientWidth ?? 240, viewport.current?.clientHeight ?? 240));
    }).catch((reason: unknown) => {
      if (active) setError(errorMessage(reason, "Das gespeicherte Blender-Bild ist nicht verfügbar."));
    });
    return () => { active = false; controller.abort(); release(); };
  }, [file.id, file.originalFilename]);

  useEffect(() => {
    if (!image || !viewport.current) return;
    image.alt = `Gespeichertes Blender-Vorschaubild: ${file.originalFilename}`;
    viewport.current.appendChild(image);
    return () => image.remove();
  }, [image, file.originalFilename]);

  useEffect(() => {
    if (!image) return;
    image.style.width = `${image.naturalWidth * zoom}px`; image.style.height = `${image.naturalHeight * zoom}px`;
    image.style.imageRendering = pixel ? "pixelated" : "auto";
  }, [image, zoom, pixel]);

  return <>
    <div ref={viewport} className="image-viewport transparency-background" />
    {!image && !error && <p className="detail-hint" role="status">Gespeichertes Blender-Bild wird geprüft …</p>}
    {error && <p className="inline-error" role="alert">{error}</p>}
    <p className="detail-hint">Gespeichertes Vorschaubild, keine 3D-Vorschau.</p>
    {image && <>
      <p className="detail-hint">{image.naturalWidth} × {image.naturalHeight} Pixel · Transparenz auf Schachbrett</p>
      <div className="visual-controls">
        <label>Zoom · {Math.round(zoom * 100)} %<input aria-label="Blender-Bildzoom" type="range" min="0.01" max="8" step="0.01" value={zoom} onChange={(event) => setZoom(Number(event.target.value))} /></label>
        <div className="visual-buttons"><button className="button outlined" onClick={fit}>Einpassen</button><button className="button outlined" onClick={() => setZoom(1)}>100 %</button></div>
        <label className="checkbox-label"><input type="checkbox" checked={pixel} onChange={(event) => setPixel(event.target.checked)} />Pixelansicht</label>
      </div>
    </>}
  </>;
}
