import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { fileSrc } from "./fileSrc.ts";
import { imageFit, spriteSheetFrame, type SpriteSheetLayout } from "./imageWorkflows";
import { imagePreviewUrl, loadPreviewImage } from "./imagePreviewUrl.ts";
import GifPreview from "./GifPreview.tsx";
import { open } from "@tauri-apps/plugin-dialog";
import { deliverSpriteExport, spriteExportPlan, packedSpriteExportPlan, type SpriteExportMode } from "./spriteExport.ts";
import type { ModelResources } from "./loadModel";
import AsepritePreview from "./AsepritePreview.tsx";
import { errorMessage } from "./errorMessage.ts";

export function ImageThumbnail({ fileId, available, filename }: { fileId: number; available: boolean; filename: string }) {
  const [url, setUrl] = useState<string | null>(null);
  useEffect(() => {
    let active = true;
    const controller = new AbortController();
    let release = () => {};
    setUrl(null);
    if (available) void invoke<string>("resolve_preview", { fileId }).then(async (path) => {
      const preview = await imagePreviewUrl(fileSrc(path), filename, controller.signal, 512);
      if (!active) { preview.release(); return; }
      release = preview.release;
      setUrl(preview.url);
    }).catch(() => {});
    return () => { active = false; controller.abort(); release(); };
  }, [fileId, available, filename]);
  const aseprite = /\.(ase|aseprite)$/i.test(filename);
  return url ? <><img className="asset-thumbnail" src={url} alt="" draggable={false} title={aseprite ? "Aseprite · Erstes vollständiges Bild" : /\.gif$/i.test(filename) ? "GIF · Erstes Bild" : undefined} loading="lazy" onError={() => setUrl(null)} />{aseprite && <span className="model-thumbnail-status">Erstes Bild</span>}</> : <span aria-hidden="true">▧</span>;
}

export default function ImagePreview({ file }: { file: { id: number; originalFilename: string } }) {
  return /\.(ase|aseprite)$/i.test(file.originalFilename) ? <AsepritePreview key={file.id} file={file} /> : /\.gif$/i.test(file.originalFilename) ? <GifPreview key={file.id} file={file} /> : <StaticImagePreview file={file} />;
}

function StaticImagePreview({ file }: { file: { id: number; originalFilename: string } }) {
  const viewport = useRef<HTMLDivElement>(null);
  const canvas = useRef<HTMLCanvasElement>(null);
  const [image, setImage] = useState<HTMLImageElement | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [zoom, setZoom] = useState(1);
  const [pixel, setPixel] = useState(true);
  const [layout, setLayout] = useState<SpriteSheetLayout>({ columns: 1, rows: 1, left: 0, top: 0, right: 0, bottom: 0, gapX: 0, gapY: 0 });
  const [fps, setFps] = useState(8);
  const [frame, setFrame] = useState(0);
  const [playing, setPlaying] = useState(false);
  const [snapshot, setSnapshot] = useState<ModelResources | null>(null);
  const [exporting, setExporting] = useState(false);
  const [exportStatus, setExportStatus] = useState<string | null>(null);
  const exportJob = useRef<{ controller: AbortController; snapshotId: string; cancellation: Promise<void> | null } | null>(null);
  const grid = image ? spriteSheetFrame(image.naturalWidth, image.naturalHeight, layout, 0) : null;
  const validGrid = grid !== null;
  const validFps = Number.isInteger(fps) && fps >= 1 && fps <= 60;
  const count = grid?.count ?? 1;
  const crop = image && grid ? spriteSheetFrame(image.naturalWidth, image.naturalHeight, layout, frame % count) : null;
  const width = crop?.width ?? image?.naturalWidth ?? 1;
  const height = crop?.height ?? image?.naturalHeight ?? 1;

  function changeLayout(field: keyof SpriteSheetLayout, value: number) {
    setLayout((current) => ({ ...current, [field]: value }));
    setFrame(0); setPlaying(false);
  }

  function fit() {
    setZoom(imageFit(width, height, viewport.current?.clientWidth ?? 240, viewport.current?.clientHeight ?? 240));
  }

  useEffect(() => {
    let active = true;
    const controller = new AbortController();
    let release = () => {};
    setImage(null); setSnapshot(null); setError(null); setExportStatus(null); setExporting(false); setPlaying(false); setFrame(0);
    exportJob.current = null;
    let snapshotId: string | null = null;
    if (canvas.current) canvas.current.width = canvas.current.height = 1;
    void invoke<ModelResources>("sheet_resources", { fileId: file.id }).then(async (prepared) => {
      if (!active) return;
      snapshotId = prepared.snapshotId;
      if (prepared.errors.length || prepared.resources.length !== 1 || prepared.resources[0].relativePath !== prepared.entryPath) throw new Error("Keine eindeutig vorbereitete Bilddatei verfügbar.");
      const preview = await loadPreviewImage(fileSrc(prepared.resources[0].path), file.originalFilename, controller.signal);
      if (!active) { preview.release(); return; }
      release = preview.release;
      setSnapshot(prepared);
      setImage(preview.image);
      setZoom(imageFit(preview.image.naturalWidth, preview.image.naturalHeight, viewport.current?.clientWidth ?? 240, viewport.current?.clientHeight ?? 240));
    }).catch((reason: unknown) => {
      if (active) setError(errorMessage(reason, "Die Bilddatei ist nicht verfügbar."));
    });
    return () => { active = false; controller.abort(); exportJob.current?.controller.abort(); exportJob.current = null; if (snapshotId) void invoke("cancel_sprite_export", { snapshotId, dispose: true }).catch(() => {}); release(); };
  }, [file.id, file.originalFilename]);

  function cancelExport(job: NonNullable<typeof exportJob.current>, message: string) {
    if (exportJob.current !== job || job.cancellation) return;
    setExportStatus(message);
    // A delayed cancellation must settle before another export can reuse this sheet pin.
    job.cancellation = Promise.resolve().then(() => invoke<void>("cancel_sprite_export", { snapshotId: job.snapshotId, dispose: false })).catch((reason: unknown) => {
      if (exportJob.current === job) {
        const detail = errorMessage(reason, "Die native Bestätigung ist nicht verfügbar.");
        setExportStatus(`Abbruch konnte nicht bestätigt werden: ${detail} Prüfe den Zielordner vor einem erneuten Export.`);
      }
    });
    job.controller.abort();
  }

  async function exportSheet(mode: SpriteExportMode) {
    if (!image || !snapshot || exportJob.current) return;
    const controller = new AbortController();
    const job = { controller, snapshotId: snapshot.snapshotId, cancellation: null as Promise<void> | null };
    exportJob.current = job;
    setPlaying(false); setExporting(true); setExportStatus(null);
    let timer: ReturnType<typeof setTimeout> | undefined;
    try {
      const plan = mode === "packed" ? () => packedSpriteExportPlan(image, { ...layout }, fps, controller.signal) : spriteExportPlan(image.naturalWidth, image.naturalHeight, layout, fps, mode);
      const path = await deliverSpriteExport(image, plan, { snapshotId: snapshot.snapshotId, fileId: file.id, filename: file.originalFilename }, controller.signal,
        async () => {
          const parent = await open({ directory: true, multiple: false, title: "Projektordner für Sprite-Export auswählen" });
          if (typeof parent === "string" && !controller.signal.aborted) timer = setTimeout(() => {
            cancelExport(job, "Zeitlimit von 30 Sekunden erreicht. Ein bereits gespeicherter Export bleibt erhalten.");
          }, 30_000);
          return parent;
        },
        (bytes) => invoke<{ status: string; path?: string }>("export_sprite", bytes));
      if (path && exportJob.current === job) setExportStatus(`Sprite-Export gespeichert: ${path}`);
    } catch (reason) {
      if (exportJob.current === job && !controller.signal.aborted) setExportStatus(errorMessage(reason, "Sprite-Export fehlgeschlagen."));
    } finally {
      clearTimeout(timer);
      if (job.cancellation) await job.cancellation;
      if (exportJob.current === job) { exportJob.current = null; setExporting(false); }
    }
  }

  useEffect(() => {
    if (!image || !canvas.current) return;
    const context = canvas.current.getContext("2d");
    if (!context) { setError("Die Bildvorschau ist in dieser Ansicht nicht verfügbar."); return; }
    canvas.current.width = width;
    canvas.current.height = height;
    context.imageSmoothingEnabled = !pixel;
    context.clearRect(0, 0, width, height);
    context.drawImage(image, crop?.x ?? 0, crop?.y ?? 0, width, height, 0, 0, width, height);
  }, [image, width, height, pixel, crop?.x, crop?.y]);

  useEffect(() => {
    if (!playing || !validGrid || !validFps || count < 2) return;
    const timer = window.setInterval(() => setFrame((current) => (current + 1) % count), 1000 / fps);
    return () => window.clearInterval(timer);
  }, [playing, validGrid, validFps, count, fps]);

  return <>
    <div ref={viewport} className="image-viewport transparency-background">
      <canvas ref={canvas} aria-label={`Bildvorschau: ${file.originalFilename}`}
        style={{ width: width * zoom, height: height * zoom, imageRendering: pixel ? "pixelated" : "auto" }} />
    </div>
    {!image && !error && <p className="detail-hint" role="status">Bild wird geladen …</p>}
    {error && <p className="inline-error" role="alert">{error}</p>}
    {image && <>
      <p className="detail-hint">{image.naturalWidth} × {image.naturalHeight} Pixel · Transparenz auf Schachbrett</p>
      <div className="visual-controls">
        <label>Zoom · {Math.round(zoom * 100)} %<input aria-label="Bildzoom" type="range" min="0.01" max="8" step="0.01" value={zoom}
          onChange={(event) => setZoom(Number(event.target.value))} /></label>
        <div className="visual-buttons"><button className="button outlined" onClick={fit}>Einpassen</button>
          <button className="button outlined" onClick={() => setZoom(1)}>100 %</button></div>
        <label className="checkbox-label"><input type="checkbox" checked={pixel} onChange={(event) => setPixel(event.target.checked)} />Pixelansicht</label>
      </div>
      <details className="sprite-controls" onToggle={(event) => { if (!event.currentTarget.open) setPlaying(false); }}><summary>Spritesheet-Rasteranimation</summary>
        <fieldset disabled={exporting} style={{ border: 0, padding: 0, margin: 0 }}><div className="sprite-grid-controls">
          <label>Spalten<input aria-label="Spritesheet-Spalten" type="number" min="1" max={image.naturalWidth} step="1" value={layout.columns} onChange={(event) => changeLayout("columns", Number(event.target.value))} /></label>
          <label>Zeilen<input aria-label="Spritesheet-Zeilen" type="number" min="1" max={image.naturalHeight} step="1" value={layout.rows} onChange={(event) => changeLayout("rows", Number(event.target.value))} /></label>
          <label>Rand links<input aria-label="Spritesheet-Rand-links" type="number" min="0" max={image.naturalWidth} step="1" value={layout.left} onChange={(event) => changeLayout("left", Number(event.target.value))} /></label>
          <label>Rand oben<input aria-label="Spritesheet-Rand-oben" type="number" min="0" max={image.naturalHeight} step="1" value={layout.top} onChange={(event) => changeLayout("top", Number(event.target.value))} /></label>
          <label>Rand rechts<input aria-label="Spritesheet-Rand-rechts" type="number" min="0" max={image.naturalWidth} step="1" value={layout.right} onChange={(event) => changeLayout("right", Number(event.target.value))} /></label>
          <label>Rand unten<input aria-label="Spritesheet-Rand-unten" type="number" min="0" max={image.naturalHeight} step="1" value={layout.bottom} onChange={(event) => changeLayout("bottom", Number(event.target.value))} /></label>
          <label>Abstand horizontal<input aria-label="Spritesheet-Abstand-X" type="number" min="0" max={image.naturalWidth} step="1" value={layout.gapX} onChange={(event) => changeLayout("gapX", Number(event.target.value))} /></label>
          <label>Abstand vertikal<input aria-label="Spritesheet-Abstand-Y" type="number" min="0" max={image.naturalHeight} step="1" value={layout.gapY} onChange={(event) => changeLayout("gapY", Number(event.target.value))} /></label>
          <label>FPS<input aria-label="Spritesheet-FPS" type="number" min="1" max="60" step="1" value={fps} onChange={(event) => {
            setFps(Number(event.target.value)); setPlaying(false);
          }} /></label>
        </div>
        {!validGrid && <p className="inline-error" role="alert">Wähle positive Zeilen/Spalten und nicht negative ganze Ränder/Abstände, die das Bild in ganze Zellen teilen.</p>}
        {!validFps && <p className="inline-error" role="alert">Wähle eine ganze Bildrate zwischen 1 und 60 FPS.</p>}
        <div className="visual-buttons"><button className="button outlined" disabled={!validGrid || !validFps || count < 2} onClick={() => setPlaying((current) => !current)}>{playing ? "Pause" : "Animation abspielen"}</button>
          <button className="button outlined" disabled={!validGrid || count < 2} onClick={() => { setPlaying(false); setFrame((current) => (current + 1) % count); }}>Nächstes Bild</button></div>
        <p className="detail-hint" role="status">{crop ? `Bild ${frame % count + 1} / ${count} · ${crop.width} × ${crop.height} Pixel pro Bild · Ursprung (${crop.x}, ${crop.y})` : "Kein gültiger Bildausschnitt · vollständiges Bild als Vorschau"}</p>
        <div className="visual-buttons"><button className="button outlined" disabled={!snapshot || !validGrid || !validFps} onClick={() => void exportSheet("frames")}>PNG-Bilder exportieren</button><button className="button outlined" disabled={!snapshot || !validGrid || !validFps} onClick={() => void exportSheet("atlas")}>Atlas exportieren</button><button className="button outlined" disabled={!snapshot || !validGrid || !validFps} onClick={() => void exportSheet("packed")}>Gepackten Atlas exportieren</button></div>
        <p className="detail-hint">Export in einen neuen Ordner, maximal 256 Bilder und 8 MiPixel.</p></fieldset>
        {exporting && <><p className="detail-hint" role="status">Sprite-Export wird vorbereitet …</p><button className="button outlined" onClick={() => {
          if (exportJob.current) cancelExport(exportJob.current, "Abbruch angefordert. Ein bereits gespeicherter Export bleibt erhalten.");
        }}>Export abbrechen</button></>}
        {exportStatus && <p className="detail-hint" role="status">{exportStatus}</p>}
      </details>
    </>}
  </>;
}
