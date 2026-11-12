import { readBlendThumbnail } from "./blendThumbnail.ts";
import { errorMessage } from "./errorMessage.ts";

const worker = globalThis as unknown as {
  onmessage: (event: MessageEvent<ArrayBuffer>) => void;
  postMessage: (message: unknown, transfer: Transferable[]) => void;
};
worker.onmessage = ({ data }) => {
  try {
    const decoded = readBlendThumbnail(data);
    worker.postMessage(decoded, [decoded.buffer]);
  } catch (reason) {
    worker.postMessage({ error: errorMessage(reason, "Blender-Vorschaubild ist nicht lesbar.") }, []);
  }
};
