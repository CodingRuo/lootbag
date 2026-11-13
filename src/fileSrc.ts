import { convertFileSrc } from "@tauri-apps/api/core";

let generation = 0;

// A rescan can replace bytes behind an unchanged path; a new query string makes the webview fetch them again.
export function bumpPreviewGeneration() {
  generation++;
}

export const fileSrc = (path: string) => (generation ? `${convertFileSrc(path)}?v=${generation}` : convertFileSrc(path));
