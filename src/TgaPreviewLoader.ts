import { DataTexture, LinearFilter, LinearMipmapLinearFilter, Loader, type LoadingManager } from "three";
import { decodeAsepriteUrl, decodeGifUrl, decodeTgaUrl } from "./imagePreviewUrl.ts";

export class ImageTexturePreviewLoader extends Loader<DataTexture> {
  private source: AbortSignal;
  private managerSignal: AbortSignal;
  private budget: { pixels: number };
  private reportFailure?: (reason: unknown) => void;
  private format: "TGA" | "GIF" | "ASEPRITE";

  constructor(manager: LoadingManager, signal: AbortSignal, format: "TGA" | "GIF" | "ASEPRITE", reportFailure?: (reason: unknown) => void, budget = { pixels: 0 }) {
    super(manager);
    this.source = signal;
    this.reportFailure = reportFailure;
    this.format = format;
    this.budget = budget;
    this.managerSignal = manager.abortController.signal;
  }

  load(url: string, onLoad?: (texture: DataTexture) => void, _onProgress?: (event: ProgressEvent) => void, onError?: (reason: unknown) => void) {
    const texture = new DataTexture();
    // FBX restores the handler's temporary path immediately after this call.
    const resource = (this.path || "") + url;
    const controller = new AbortController();
    const abort = () => controller.abort();
    for (const signal of [this.source, this.managerSignal]) {
      if (signal.aborted) abort();
      else signal.addEventListener("abort", abort, { once: true });
    }
    this.manager.itemStart(resource);
    let retained = 0;
    void (async () => {
      try {
        controller.signal.throwIfAborted();
        const transport = this.manager.resolveURL(resource);
        const decoded = await (this.format === "ASEPRITE" ? decodeAsepriteUrl : this.format === "GIF" ? decodeGifUrl : decodeTgaUrl)(transport, controller.signal);
        controller.signal.throwIfAborted();
        retained = decoded.width * decoded.height;
        if (this.budget.pixels + retained > 32 * 1024 * 1024) { retained = 0; throw new Error("Dekodierte Modelltexturen überschreiten zusammen 32 Megapixel."); }
        this.budget.pixels += retained;
        texture.image = { data: new Uint8Array(decoded.buffer), width: decoded.width, height: decoded.height };
        texture.flipY = true;
        texture.magFilter = LinearFilter;
        texture.minFilter = LinearMipmapLinearFilter;
        texture.generateMipmaps = true;
        texture.needsUpdate = true;
        onLoad?.(texture);
      } catch (reason) {
        this.budget.pixels -= retained;
        texture.image.data = null;
        texture.image.width = texture.image.height = 0;
        try { this.manager.itemError(resource); this.reportFailure?.(reason); } finally { onError?.(reason); }
      } finally {
        for (const signal of [this.source, this.managerSignal]) signal.removeEventListener("abort", abort);
        this.manager.itemEnd(resource);
      }
    })().catch(() => {});
    return texture;
  }
}
