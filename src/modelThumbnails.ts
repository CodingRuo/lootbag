export type ModelThumbnailResult = { url: string | null; warning: string | null; error: string | null };
import { errorMessage } from "./errorMessage.ts";
type Listener = (result: ModelThumbnailResult) => void;
type Entry = {
  key: string;
  fileId: number;
  listeners: Set<Listener>;
  result?: ModelThumbnailResult;
  controller?: AbortController;
};

export function createModelThumbnailQueue(
  render: (fileId: number, signal: AbortSignal) => Promise<ModelThumbnailResult>,
  revoke: (url: string) => void,
  disposeRenderer: () => void,
  cacheLimit = 120,
) {
  let scope: string | null = null;
  let running = false;
  const entries = new Map<string, Entry>();
  const pending: Entry[] = [];

  function release(entry: Entry) {
    entry.controller?.abort();
    if (entry.result?.url) revoke(entry.result.url);
    entries.delete(entry.key);
    const queued = pending.indexOf(entry);
    if (queued !== -1) pending.splice(queued, 1);
  }

  function trim() {
    let cached = [...entries.values()].filter((entry) => entry.result).length;
    for (const entry of entries.values()) {
      if (cached <= cacheLimit) break;
      if (entry.result && !entry.listeners.size) { release(entry); cached--; }
    }
  }

  async function pump() {
    if (running) return;
    running = true;
    try {
      while (pending.length) {
        const entry = pending.shift()!;
        if (entries.get(entry.key) !== entry || !entry.listeners.size) continue;
        const controller = new AbortController();
        entry.controller = controller;
        let result: ModelThumbnailResult;
        try {
          result = await render(entry.fileId, controller.signal);
        } catch (reason) {
          result = {
            url: null, warning: null,
            error: errorMessage(reason, "Modellvorschau nicht verfügbar."),
          };
        }
        if (controller.signal.aborted || entries.get(entry.key) !== entry) {
          if (result.url) revoke(result.url);
          continue;
        }
        entry.controller = undefined;
        entry.result = result;
        for (const listener of entry.listeners) listener(result);
        trim();
      }
    } finally {
      running = false;
    }
  }

  function reset(nextScope: string | null) {
    if (scope === nextScope) return;
    scope = nextScope;
    for (const entry of entries.values()) release(entry);
    pending.length = 0;
  }

  return {
    reset,
    request(nextScope: string, key: string, fileId: number, listener: Listener) {
      reset(nextScope);
      let entry = entries.get(key);
      if (!entry) {
        entry = { key, fileId, listeners: new Set() };
        entries.set(key, entry);
        pending.push(entry);
      } else {
        entries.delete(key);
        entries.set(key, entry);
      }
      entry.listeners.add(listener);
      if (entry.result) listener(entry.result);
      void pump();
      const subscribed = entry;
      return () => {
        subscribed.listeners.delete(listener);
        if (!subscribed.listeners.size && !subscribed.result && entries.get(key) === subscribed) release(subscribed);
        trim();
      };
    },
    dispose() {
      reset(null);
      disposeRenderer();
    },
  };
}

export type ModelThumbnailQueue = ReturnType<typeof createModelThumbnailQueue>;
