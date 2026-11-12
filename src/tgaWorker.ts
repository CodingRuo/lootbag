import { decodeTga } from "./tgaDecode.ts";

const worker = globalThis as unknown as {
  onmessage: (event: MessageEvent<ArrayBuffer>) => void;
  postMessage: (message: unknown, transfer: Transferable[]) => void;
};
worker.onmessage = ({ data }) => {
  try {
    const decoded = decodeTga(data);
    worker.postMessage({ width: decoded.width, height: decoded.height, buffer: decoded.data.buffer }, [decoded.data.buffer]);
  } catch (reason) {
    worker.postMessage({ error: reason instanceof Error ? reason.message : "TGA kann nicht dekodiert werden." }, []);
  }
};
