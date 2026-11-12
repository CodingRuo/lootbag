import { useEffect, useRef, useState } from "react";
import { Channel, invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";

export type RecoveryKind = "backup" | "restore";
type RecoveryReason = "busy" | "stale" | "sourceChanged" | "limit" | "unsupported" | "invalidArtifact" | "destinationChanged" | "io" | "cleanupIncomplete";
type RecoverySummary = { entries: number; files: number; directories: number; logicalBytes: number; externalReferenceRecords: number | null; externalCoverage: "metadataOnly" };
type RecoveryLimits = { maxLogicalBytes: number; maxEntries: number; maxDepth: number; maxRelativePathBytes: number; maxComponentBytes: number; maxManifestBytes: number; maxRetainedBytes: number; preparationDeadlineMs: number; runDeadlineMs: number; chunkBytes: number };
export type RecoveryPreparation =
  | { requestId: string; kind: RecoveryKind; status: "ready"; destinationPath: string; summary: RecoverySummary; limits: RecoveryLimits }
  | { requestId: string; kind: RecoveryKind; status: "unavailable"; reason: RecoveryReason; message: string }
  | { requestId: string; kind: RecoveryKind; status: "cancelled" };
export type RecoveryProgress = { requestId: string; kind: RecoveryKind; phase: "preparing" | "reserving" | "copying" | "verifyingCopy" | "verifyingSource" | "validatingCatalog" | "publishing" | "cleaningUp"; entriesDone: number; entriesTotal: number | null; bytesDone: number; bytesTotal: number | null; elapsedMs: number; currentRelativePath: string | null };
export type RecoveryOutcome =
  | { requestId: string; kind: RecoveryKind; status: "published"; destinationPath: string; summary: RecoverySummary; durability: "synced" | "uncertain"; warning: string | null }
  | { requestId: string; kind: RecoveryKind; status: "cancelled"; cleanup: "complete" }
  | { requestId: string; kind: RecoveryKind; status: "failed"; reason: RecoveryReason; message: string; cleanup: "notNeeded" | "complete" | "retained"; residuePath: string | null };
type Job = { id: string; pending: boolean; progressOwner: object | null; registered: boolean; stopped: boolean; repreparing: boolean; closing: boolean; acknowledged: boolean; cancellation: Promise<boolean> | null; cancelError: string | null };
const phases: Record<RecoveryProgress["phase"], string> = { preparing: "Quelle prüfen", reserving: "Ziel reservieren", copying: "Dateien kopieren", verifyingCopy: "Kopie überprüfen", verifyingSource: "Quelle erneut überprüfen", validatingCatalog: "Katalog überprüfen", publishing: "Neuen Zielordner veröffentlichen", cleaningUp: "Zwischendateien aufräumen" };
const message = (error: unknown) => typeof error === "string" ? error : error instanceof Error ? error.message : "Der Vorgang konnte nicht abgeschlossen werden.";

export function RecoveryDialog({ scopeId, root, kind, isCurrent, onClose, onOpenRestored, formatBytes: bytes }: { scopeId: string; root: string | null; kind: RecoveryKind; isCurrent: () => boolean; onClose: () => void; onOpenRestored: (path: string) => void; formatBytes: (value: number) => string }) {
  const dialog = useRef<HTMLDialogElement>(null);
  const mounted = useRef(true);
  const request = useRef<Job | null>(null);
  const opening = useRef<object | null>(null);
  const [preparation, setPreparation] = useState<RecoveryPreparation | null>(null);
  const [outcome, setOutcome] = useState<RecoveryOutcome | null>(null);
  const outcomeRef = useRef<RecoveryOutcome | null>(null);
  const [runError, setRunError] = useState<string | null>(null);
  const runErrorRef = useRef<string | null>(null);
  const [progress, setProgress] = useState<RecoveryProgress | null>(null);
  const [running, setRunning] = useState(false);
  const [stopping, setStopping] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [cancelError, setCancelError] = useState<string | null>(null);
  function owns(job: Job) { return mounted.current && isCurrent() && request.current === job; }
  useEffect(() => {
    mounted.current = true;
    dialog.current?.showModal();
    return () => {
      mounted.current = false;
      opening.current = null;
      const job = request.current; request.current = null;
      if (job) { job.stopped = true; job.repreparing = false; void cancel(job); }
    };
  }, [scopeId]);

  function channel(job: Job) {
    const owner = {}; job.progressOwner = owner;
    const onProgress = new Channel<RecoveryProgress>();
    onProgress.onmessage = value => { if (owns(job) && job.pending && job.progressOwner === owner && value.requestId === job.id && value.kind === kind) setProgress(value); };
    return onProgress;
  }
  function cancel(job: Job, force = false): Promise<boolean> {
    if (job.cancellation) return job.cancellation;
    if (!job.registered || (job.acknowledged && !force)) return Promise.resolve(true);
    job.acknowledged = false;
    const pending = invoke<{ requestId: string; acknowledged: true }>("cancel_library_recovery", { requestId: job.id }).then(result => {
      if (result.requestId !== job.id || result.acknowledged !== true) throw new Error("Abbruchbestätigung gehört nicht zu diesem Vorgang.");
      job.acknowledged = true; job.cancelError = null;
      if (owns(job)) setCancelError(null);
      return true;
    }).catch(error => {
      job.cancelError = message(error);
      if (owns(job)) setCancelError(job.cancelError);
      return false;
    }).finally(() => { if (job.cancellation === pending) job.cancellation = null; });
    job.cancellation = pending;
    return pending;
  }
  async function finish(job: Job) {
    if (job.cancellation) await job.cancellation;
    if (!owns(job)) return;
    if (job.stopped && job.registered && !job.acknowledged) return;
    request.current = null; setRunning(false); setStopping(false);
    // A committed destination or retained residue remains visible after a late Stop/close.
    if (job.closing && !outcomeRef.current && !runErrorRef.current) onClose();
  }
  async function prepare() {
    if (request.current?.pending || running) return;
    const older = request.current;
    if (older) {
      older.pending = true; older.stopped = true; older.repreparing = true; setRunning(true); setStopping(false);
      const acknowledged = await cancel(older); older.pending = false;
      if (!acknowledged || !owns(older)) return;
      if (!older.repreparing) { setPreparation(null); await finish(older); return; }
      request.current = null;
    }
    if (!mounted.current || !isCurrent()) return;
    const job: Job = { id: crypto.randomUUID(), pending: true, progressOwner: null, registered: false, stopped: false, repreparing: false, closing: false, acknowledged: false, cancellation: null, cancelError: null };
    request.current = job; setRunning(true); setStopping(false); setError(null); setCancelError(null); setPreparation(null); setProgress(null);
    let retainedReady = false;
    try {
      const artifact = kind === "restore" ? await open({ directory: true, multiple: false, title: "Backup-Ordner auswählen" }) : null;
      if (!owns(job) || job.stopped || (kind === "restore" && typeof artifact !== "string")) return;
      const parent = await open({ directory: true, multiple: false, title: kind === "backup" ? "Ziel für die Bibliothekssicherung auswählen" : "Ziel für die wiederhergestellte Bibliothek auswählen" });
      if (!owns(job) || job.stopped || typeof parent !== "string") return;
      job.registered = true;
      const result = await invoke<RecoveryPreparation>("prepare_library_recovery", { root, requestId: job.id, request: kind === "backup" ? { kind, destinationParent: parent } : { kind, artifactRoot: artifact, destinationParent: parent }, onProgress: channel(job) });
      if (result.requestId !== job.id || result.kind !== kind) throw new Error("Vorbereitung gehört nicht zu diesem Vorgang.");
      if (!owns(job) || job.stopped) {
        if (job.cancellation) await job.cancellation;
        await cancel(job, true);
        return;
      }
      setPreparation(result);
      if (result.status === "ready") { retainedReady = true; setRunning(false); return; }
      job.registered = false;
    } catch (error) {
      if (owns(job)) setError(message(error));
      if (job.registered) { job.stopped = true; await cancel(job); }
    } finally {
      job.pending = false; job.progressOwner = null;
      // Keep only this preparation's successfully retained owner for explicit Start.
      if (!retainedReady || job.stopped || !owns(job)) await finish(job);
    }
  }
  async function start() {
    const job = request.current;
    if (!job || job.pending || job.stopped || preparation?.status !== "ready") return;
    job.pending = true; setRunning(true); setError(null); setProgress(null);
    try {
      const result = await invoke<RecoveryOutcome>("run_library_recovery", { requestId: job.id, onProgress: channel(job) });
      if (result.requestId !== job.id || result.kind !== kind) throw new Error("Ergebnis gehört nicht zu diesem Vorgang.");
      if (owns(job)) { outcomeRef.current = result; setOutcome(result); runErrorRef.current = null; setRunError(null); setPreparation(null); }
    } catch (error) {
      if (owns(job)) { runErrorRef.current = message(error); setRunError(runErrorRef.current); }
      job.stopped = true; await cancel(job);
    }
    finally { job.pending = false; job.progressOwner = null; await finish(job); }
  }
  async function stop(close = false) {
    opening.current = null;
    const job = request.current;
    if (!job) { if (close) onClose(); return; }
    job.stopped = true; job.repreparing = false; job.closing ||= close; setStopping(true); setRunning(true);
    await cancel(job);
    if (!job.pending) { setPreparation(null); await finish(job); }
  }
  async function openRestored(path: string) {
    if (opening.current || running || !mounted.current || !isCurrent()) return;
    const intent = {}; opening.current = intent;
    try {
      const job = request.current;
      if (job) {
        if (job.pending || job.stopped) return;
        job.stopped = true; job.repreparing = false; setRunning(true); setStopping(true);
        await cancel(job);
        if (owns(job)) { setPreparation(null); await finish(job); }
      }
      if (opening.current === intent && mounted.current && isCurrent() && request.current === null) onOpenRestored(path);
    } finally { if (opening.current === intent) opening.current = null; }
  }

  const ready = preparation?.status === "ready" ? preparation : null;
  return <dialog ref={dialog} className="trash-dialog storage-dialog recovery-dialog" aria-labelledby="recovery-title" aria-busy={running} onCancel={event => { event.preventDefault(); void stop(true); }}>
    <div className="trash-heading"><h2 id="recovery-title">{kind === "backup" ? "Bibliothek sichern" : "Bibliothek aus Backup wiederherstellen"}</h2><button className="button outlined" onClick={() => void stop(true)}>Schließen</button></div>
    <p className="detail-hint">{kind === "backup" ? "Sichert die lokale Bibliothek in einem neuen Backup-Ordner." : "Stellt ein Backup in einem neuen Bibliotheksordner wieder her. Bestehende Ordner werden nicht überschrieben. Die Bibliothek wird erst auf deinen Wunsch geöffnet."}</p>
    {root && <p className="source-path">Aktuelle Bibliothek: {root}</p>}
    <div className="recovery-actions"><button className="button outlined" disabled={running || (kind === "backup" && root === null)} onClick={() => void prepare()}>{preparation || outcome ? "Erneut vorbereiten" : "Vorbereiten"}</button>
      {ready && <button className="button primary" disabled={running} onClick={() => void start()}>Starten</button>}
      {running && <button className="button outlined" disabled={stopping && !cancelError} onClick={() => void stop()}>{cancelError ? "Abbruch erneut versuchen" : stopping ? "Abbruch angefordert …" : "Stoppen"}</button>}</div>
    {running && <div role="status" aria-live="polite"><p>{stopping ? "Abbruch angefordert. Warte auf Vorgang und Bestätigung …" : progress ? phases[progress.phase] : "Auswahl oder Vorbereitung läuft …"}</p>
      {progress && <><progress aria-label={phases[progress.phase]} value={progress.entriesTotal === null ? undefined : progress.entriesDone} max={progress.entriesTotal === null ? undefined : Math.max(1, progress.entriesTotal)} />
      <p>{progress.entriesDone.toLocaleString("de-DE")} / {progress.entriesTotal?.toLocaleString("de-DE") ?? "unbekannt"} Einträge · {bytes(progress.bytesDone)} / {progress.bytesTotal === null ? "unbekannt" : bytes(progress.bytesTotal)}</p><p className="source-path">{progress.currentRelativePath}</p></>}
    </div>}
    {error && <p role="alert" className="inline-error">{error}</p>}
    {cancelError && <p role="alert" className="inline-error">Abbruch nicht bestätigt: {cancelError}</p>}
    {preparation?.status === "unavailable" && <p role="alert" className="inline-error">Vorbereitung nicht verfügbar ({preparation.reason}): {preparation.message}</p>}
    {preparation?.status === "cancelled" && <p role="status">Vorbereitung abgebrochen.</p>}
    {ready && <><h3>Vorbereitet – vor dem Start prüfen</h3><p className="detail-hint">Diese Vorbereitung bestätigt den vorgesehenen Umfang. Die Kopie wird erst beim Start erstellt und überprüft.</p><p className="source-path">Neuer Zielordner: {ready.destinationPath}</p>
      <p>{ready.summary.files.toLocaleString("de-DE")} Dateien · {ready.summary.directories.toLocaleString("de-DE")} Ordner · {bytes(ready.summary.logicalBytes)} logische Dateibytes</p>
      <p className="detail-hint">Verwaltete Originale, Begleitdateien und Lizenzen, leere Ordner, Papierkorb und Katalog mit Anmerkungen und Projektordnern werden übernommen.</p>
      <p>Externe Referenzen: {ready.summary.externalReferenceRecords?.toLocaleString("de-DE") ?? "Anzahl unbekannt"} – nur Katalogmetadaten. Externe Dateien werden nicht kopiert oder neu verknüpft.</p>
      <p className="detail-hint">Während des Laufs sind Bibliotheksänderungen blockiert. Dateien werden mehrfach gelesen, um Kopie und Quelle zu überprüfen. Logische Bytes garantieren keinen freien Speicherplatz. Abgebrochene Läufe können nicht fortgesetzt werden.</p>
      <details><summary>Grenzen dieses Sicherungsformats</summary><ul>
        <li>Bis {bytes(ready.limits.maxLogicalBytes)} und {ready.limits.maxEntries.toLocaleString("de-DE")} Einträge; Tiefe {ready.limits.maxDepth}.</li>
        <li>Relative Pfade bis {ready.limits.maxRelativePathBytes} Bytes; Namen bis {ready.limits.maxComponentBytes} Bytes.</li>
        <li>Manifest bis {bytes(ready.limits.maxManifestBytes)}, vorgehaltene Daten bis {bytes(ready.limits.maxRetainedBytes)}, Kopierblöcke {bytes(ready.limits.chunkBytes)}.</li>
        <li>Vorbereitung bis {ready.limits.preparationDeadlineMs / 60000} Minuten; Lauf bis {ready.limits.runDeadlineMs / 3600000} Stunden.</li>
        <li>Hardlink-Struktur, ACLs, erweiterte Attribute und Zeitstempel werden nicht erhalten. Ausgaben sind für den Besitzer lesbar.</li>
        <li>Verknüpfungen, aktive Zwischendateien oder nicht unterstützte Katalogzustände können den Vorgang verhindern.</li>
      </ul></details></>}
    {(outcome || runError) && <section className="recovery-result" aria-label="Letztes Ergebnis"><h3>Letztes Ergebnis</h3>
      {runError && <p role="alert" className="inline-error">Ausführung nicht bestätigt: {runError}</p>}
      {outcome && <>
      {outcome.status === "published" ? <><p role="status">{outcome.kind === "backup" ? "Backup erstellt." : "Bibliothek wiederhergestellt."}</p><p className="source-path">{outcome.destinationPath}</p>
        <p>{outcome.summary.entries.toLocaleString("de-DE")} Einträge · {outcome.summary.files.toLocaleString("de-DE")} Dateien · {bytes(outcome.summary.logicalBytes)} logische Dateibytes</p>
        <p className={outcome.durability === "uncertain" ? "inline-error" : "detail-hint"}>{outcome.durability === "synced" ? "Synchronisierungsaufrufe erfolgreich. Dies ist keine Zusicherung bei Stromausfall." : "Zielordner veröffentlicht; Synchronisierung nicht vollständig bestätigt."}</p>
        {outcome.warning && <p role="alert" className="inline-error">{outcome.warning}</p>}
        {outcome.kind === "restore" && <button className="button outlined" disabled={running} onClick={() => void openRestored(outcome.destinationPath)}>Wiederhergestellte Bibliothek öffnen</button>}</>
      : outcome.status === "cancelled" ? <p role="status">Vorgang abgebrochen. Zwischendateien aufgeräumt.</p>
      : <><p role="alert" className="inline-error">Vorgang fehlgeschlagen ({outcome.reason}): {outcome.message}</p><p>{outcome.cleanup === "retained" ? "Zwischendateien wurden behalten. Bitte diesen Pfad aufbewahren." : outcome.cleanup === "complete" ? "Zwischendateien aufgeräumt." : "Keine Zwischendateien aufzuräumen."}</p>{outcome.residuePath && <p className="source-path">{outcome.residuePath}</p>}</>}
      </>}
    </section>}
  </dialog>;
}
