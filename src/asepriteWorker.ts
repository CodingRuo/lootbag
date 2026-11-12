import { decodeAseprite } from "./asepriteDecode.ts";
import { errorMessage } from "./errorMessage.ts";

const worker = globalThis as unknown as {
  onmessage: (event: MessageEvent<ArrayBuffer | { buffer: ArrayBuffer; animation: true }>) => void;
  postMessage: (message: unknown, transfer: Transferable[]) => void;
};
worker.onmessage = ({ data }) => {
  const animation = !(data instanceof ArrayBuffer);
  void decodeAseprite(animation ? data.buffer : data, undefined, !animation).then((decoded) => {
    if (animation) worker.postMessage(decoded, decoded.frames.map((frame) => frame.buffer));
    else worker.postMessage({ width: decoded.width, height: decoded.height, buffer: decoded.frames[0].buffer }, [decoded.frames[0].buffer]);
  }, (reason: unknown) => {
    worker.postMessage({ error: errorMessage(reason, "Aseprite kann nicht dekodiert werden.") }, []);
  });
};
