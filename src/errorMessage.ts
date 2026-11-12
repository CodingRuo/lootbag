export function errorMessage(error: unknown, fallback = "Der Vorgang konnte nicht abgeschlossen werden.") {
  return typeof error === "string" ? error : error instanceof Error ? error.message : fallback;
}
