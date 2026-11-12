import { useEffect, useRef, useState, type ReactNode } from "react";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { PerspectiveCamera, WebGLRenderer } from "three";
import { fitModelCamera, loadModel, modelScene, modelTransportUrl, type ModelResources } from "./loadModel";
import { createModelThumbnailQueue, type ModelThumbnailQueue, type ModelThumbnailResult } from "./modelThumbnails";
import { blendEntryResource } from "./modelResources.ts";
import { imagePreviewUrl } from "./imagePreviewUrl.ts";

export function createCatalogModelThumbnails() {
  let renderer: WebGLRenderer | null = null;
  return createModelThumbnailQueue(async (fileId, signal) => {
    const snapshot = await invoke<ModelResources>("model_thumbnail_resources", { fileId });
    signal.throwIfAborted();
    if (snapshot.format === "BLEND") {
      const entry = blendEntryResource(snapshot);
      const preview = await imagePreviewUrl(modelTransportUrl(convertFileSrc(entry.path), snapshot.snapshotId), entry.relativePath, signal, 192);
      if (signal.aborted) { preview.release(); signal.throwIfAborted(); }
      return { url: preview.url, warning: ["Gespeichertes Blender-Bild; kann älter als die Szene sein. Keine interaktive 3D-Vorschau.", ...snapshot.errors].join("\n"), error: null };
    }
    const loaded = await loadModel(snapshot, signal);
    let view: ReturnType<typeof modelScene> | null = null;
    try {
      signal.throwIfAborted();
      view = modelScene(loaded);
      if (!renderer) {
        renderer = new WebGLRenderer({ antialias: true });
        renderer.setPixelRatio(1);
        renderer.setSize(192, 144, false);
      }
      const camera = new PerspectiveCamera(45, 192 / 144, 0.01, 1000);
      fitModelCamera(camera, loaded.model);
      renderer.render(view.scene, camera);
      const png = await new Promise<Blob>((resolve, reject) => renderer!.domElement.toBlob((blob) => {
        if (blob) resolve(blob);
        else reject(new Error("Das Modell-Vorschaubild konnte nicht erstellt werden."));
      }, "image/png"));
      signal.throwIfAborted();
      const warnings = [...loaded.warnings, ...loaded.replacements.map((path) => `Texturersatz nur für die Vorschau: ${path}`)];
      return { url: URL.createObjectURL(png), warning: warnings.length ? warnings.join("\n") : null, error: null };
    } finally {
      view?.dispose();
      loaded.dispose();
      renderer?.renderLists.dispose();
    }
  }, (url) => URL.revokeObjectURL(url), () => {
    renderer?.dispose();
    renderer?.forceContextLoss();
    renderer = null;
  });
}

export default function ModelThumbnail({ queue, scope, fileId, format, available, fallback }: {
  queue: ModelThumbnailQueue;
  scope: string;
  fileId: number;
  format: string;
  available: boolean;
  fallback: ReactNode;
}) {
  const host = useRef<HTMLSpanElement>(null);
  const [result, setResult] = useState<ModelThumbnailResult | null>(null);

  useEffect(() => {
    setResult(null);
    if (!available || !host.current) return;
    let unsubscribe: (() => void) | null = null;
    let active = true;
    const observer = new IntersectionObserver(([entry]) => {
      if (entry.isIntersecting && !unsubscribe) {
        unsubscribe = queue.request(scope, `${fileId}:${available}`, fileId, (value) => { if (active) setResult(value); });
      } else if (!entry.isIntersecting && unsubscribe) {
        unsubscribe();
        unsubscribe = null;
      }
    }, { root: host.current.closest(".catalog-scroll") });
    observer.observe(host.current);
    return () => { active = false; observer.disconnect(); unsubscribe?.(); };
  }, [queue, scope, fileId, available]);

  const message = !available ? "Modelldatei nicht verfügbar." : result?.error ?? result?.warning;
  return <span ref={host} className="model-thumbnail" title={message ?? undefined}>
    {result?.url ? <img className="model-thumbnail-image" src={result.url} alt="" onError={() => setResult({ url: null, warning: null, error: "Modell-Vorschaubild nicht verfügbar." })} /> : fallback}
    {message && <span className="model-thumbnail-status" aria-label={message}>{result?.error || !available ? "Keine Vorschau" : format === "BLEND" ? "Gespeichertes Bild" : "Textur fehlt/ersetzt"}</span>}
  </span>;
}
