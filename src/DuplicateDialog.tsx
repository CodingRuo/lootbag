import { useEffect, useRef, useState } from "react";
import { Channel, invoke } from "@tauri-apps/api/core";

type DuplicateProgress = {
  stage: "hash" | "compare" | "verify" | "finished";
  total: number;
  processed: number;
  bytesRead: number;
};
type DuplicateFile = {
  fileId: number;
  assetId: number;
  packageId: number;
  packageName: string;
  originalFilename: string;
  relativePath: string;
  format: string;
  sizeBytes: number;
  status: "checked" | "unavailable" | "notChecked";
  sha256: string | null;
  reason: string | null;
};
type DuplicateGroup = { sha256: string; sizeBytes: number; members: number[]; sharedSource: boolean };
type DuplicateReport = {
  complete: boolean;
  cancelled: boolean;
  startedAtMs: number;
  finishedAtMs: number;
  total: number;
  processed: number;
  bytesRead: number;
  groups: DuplicateGroup[];
  files: DuplicateFile[];
  issues: string[];
};
type Job = {
  id: string;
  label: string;
  phase: "prepare" | "ready" | "run" | "settling";
  stopping: boolean;
  cancellation: Promise<void> | null;
  cancellationError: string | null;
};
const errorText = (error: unknown) => typeof error === "string" ? error : error instanceof Error ? error.message : "Prüfung konnte nicht abgeschlossen werden.";
const stageLabels = { hash: "Dateien werden gehasht", compare: "Bytevergleich läuft", verify: "Quellen und Katalog werden überprüft", finished: "Prüfung wird abgeschlossen" };
const fileLabels = { checked: "Geprüft", unavailable: "Nicht verfügbar", notChecked: "Nicht geprüft" };
const date = new Intl.DateTimeFormat("de-DE", { dateStyle: "short", timeStyle: "medium" });
function pagination(name: string, offset: number, total: number, limit: number, change: (offset: number) => void) {
  return <nav className="trash-pagination" aria-label={`${name} blättern`}>
    <span>{name}: {total ? offset + 1 : 0}–{Math.min(offset + limit, total)} von {total.toLocaleString("de-DE")}</span>
    <button className="button outlined" aria-label={`${name}: Zurück`} disabled={offset === 0}
      onClick={() => change(Math.max(0, offset - limit))}>Zurück</button>
    <button className="button outlined" aria-label={`${name}: Weiter`} disabled={offset + limit >= total}
      onClick={() => change(offset + limit)}>Weiter</button>
  </nav>;
}
function identity(file: DuplicateFile) { return <span className="detail-hint">Datei #{file.fileId} · Asset #{file.assetId} · Paket #{file.packageId}</span>; }

export function DuplicateDialog({ root, selectedPackage, onClose, formatBytes: bytes }: { root: string; selectedPackage: { id: number; name: string } | null; onClose: () => void; formatBytes: (bytes: number) => string }) {
  const dialog = useRef<HTMLDialogElement>(null);
  const request = useRef<Job | null>(null);
  const [scope, setScope] = useState("all");
  const [prepared, setPrepared] = useState<{ total: number; label: string } | null>(null);
  const [report, setReport] = useState<DuplicateReport | null>(null);
  const [progress, setProgress] = useState<DuplicateProgress | null>(null);
  const [running, setRunning] = useState(false);
  const [stopping, setStopping] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [groupOffset, setGroupOffset] = useState(0);
  const [groupIndex, setGroupIndex] = useState(0);
  const [memberOffset, setMemberOffset] = useState(0);
  const [fileOffset, setFileOffset] = useState(0);
  const [issueOffset, setIssueOffset] = useState(0);
  function resetPages() { setGroupOffset(0); setGroupIndex(0); setMemberOffset(0); setFileOffset(0); setIssueOffset(0); }
  useEffect(() => { dialog.current?.showModal(); }, []);
  useEffect(() => {
    setScope("all"); setPrepared(null); setReport(null); setProgress(null); setError(null); setRunning(false); setStopping(false);
    resetPages();
    return () => { const older = request.current; request.current = null; if (older) void cancel(older); };
  }, [root, selectedPackage?.id, selectedPackage?.name]);
  function cancel(job: Job) {
    job.stopping = true;
    if (!job.cancellation) job.cancellation = invoke<void>("cancel_duplicate_check", { requestId: job.id }).catch((error: unknown) => {
      job.cancellationError = `Abbruch konnte nicht bestätigt werden: ${errorText(error)}`;
      if (request.current === job) setError(job.cancellationError);
    });
    return job.cancellation;
  }
  async function finish(job: Job) {
    await job.cancellation;
    if (request.current === job) { request.current = null; setRunning(false); setStopping(false); }
  }
  function stop() {
    const job = request.current; if (!job || !running) return;
    setStopping(true); void cancel(job);
  }
  function changeScope(value: string) {
    const older = request.current; request.current = null;
    if (older) void cancel(older);
    setScope(value); setPrepared(null); setReport(null); setProgress(null); setError(null);
    resetPages();
  }
  function close() {
    const older = request.current; request.current = null;
    if (older) void cancel(older);
    onClose();
  }
  async function prepare() {
    if (running || (request.current && request.current.phase !== "ready")) return;
    const older = request.current;
    const packageId = scope === "package" ? selectedPackage?.id ?? null : null;
    const job: Job = { id: crypto.randomUUID(), label: packageId === null ? "Alle aktiven katalogisierten Dateivarianten – über alle Katalogseiten" : `Paket: ${selectedPackage!.name} – alle aktiven Dateivarianten über alle Katalogseiten`, phase: "prepare", stopping: false, cancellation: null, cancellationError: null };
    request.current = job; setRunning(true); setStopping(false); setError(null); setPrepared(null); setReport(null); setProgress(null);
    resetPages();
    try {
      if (older) await cancel(older);
      if (request.current !== job || job.stopping) return;
      const result = await invoke<{ requestId: string; total: number }>("prepare_duplicate_check", { root, requestId: job.id, packageId });
      if (request.current !== job || job.stopping) {
        // Cancellation can precede preparation's backend attachment. Discard a late successful plan too.
        await job.cancellation;
        await invoke("cancel_duplicate_check", { requestId: job.id });
        return;
      }
      if (result.requestId !== job.id) throw new Error("Vorbereiteter Umfang gehört nicht zu dieser Anfrage.");
      job.phase = "ready"; setPrepared({ total: result.total, label: job.label });
    } catch (error: unknown) { if (request.current === job) setError([job.cancellationError, errorText(error)].filter(Boolean).join(" ")); await cancel(job); }
    finally { if (job.phase === "ready" && request.current === job) setRunning(false); else { job.phase = "settling"; await finish(job); } }
  }
  async function start() {
    const job = request.current;
    if (!job || job.phase !== "ready" || job.stopping) return;
    job.phase = "run"; setRunning(true); setError(null);
    const channel = new Channel<DuplicateProgress>();
    channel.onmessage = (value) => { if (request.current === job && job.phase === "run") setProgress(value); };
    try { const result = await invoke<DuplicateReport>("run_duplicate_check", { requestId: job.id, progress: channel }); if (request.current === job) setReport(result); }
    catch (error: unknown) { if (request.current === job) setError([job.cancellationError, errorText(error)].filter(Boolean).join(" ")); }
    finally { job.phase = "settling"; await finish(job); }
  }
  const group = report?.groups[groupIndex];
  const filesById = new Map(report?.files.map((file) => [file.fileId, file]));
  return <dialog ref={dialog} className="trash-dialog duplicate-dialog" aria-labelledby="duplicate-title" onCancel={close} onClose={close}>
    <div className="trash-heading"><h2 id="duplicate-title">Datei-Duplikate prüfen</h2><button className="button outlined" onClick={close}>Schließen</button></div>
    <p className="detail-hint">Prüft aktive katalogisierte Dateivarianten über alle Katalogseiten. Such-, Kategorie-, Tag-, Projektordner- und Audiofilter werden nicht angewendet. Nicht katalogisierte Begleitdateien und Papierkorb sind ausgeschlossen.</p>
    <p className="source-path">{root}</p>
    <label>Umfang<select aria-label="Umfang der Duplikatprüfung" value={scope} disabled={running} onChange={(event) => changeScope(event.target.value)}><option value="all">Gesamte Bibliothek</option>{selectedPackage && <option value="package">Paket: {selectedPackage.name}</option>}</select></label>
    <div className="duplicate-actions"><button className="button outlined" disabled={running} onClick={() => void prepare()}>Umfang vorbereiten</button>{prepared && request.current?.phase === "ready" && <button className="button" disabled={running} onClick={() => void start()}>Prüfung starten</button>}{running && <button className="button outlined" disabled={stopping} onClick={stop}>{stopping ? "Abbruch angefordert …" : "Prüfung abbrechen"}</button>}</div>
    {prepared && <p className="detail-hint">{prepared.label}. {prepared.total.toLocaleString("de-DE")} Dateivarianten vorbereitet.</p>}
    {running && <p role="status">{stopping ? "Abbruch angefordert; Abschluss und Bestätigung werden abgewartet …" : request.current?.phase === "prepare" ? "Umfang wird vorbereitet …" : progress ? `${stageLabels[progress.stage]} …` : "Dateien werden geprüft …"} {progress && `${progress.processed} / ${progress.total} Dateien · ${bytes(progress.bytesRead)} gelesen`}</p>}
    {error && <p role="alert" className="inline-error">{error}</p>}
    {report && <><p role="status" className={report.complete ? "detail-hint" : "inline-error"}>{report.complete ? "Prüfung vollständig im vorbereiteten Umfang." : report.cancelled ? "Prüfung abgebrochen. Die Ergebnisse sind unvollständig." : "Prüfung unvollständig. Hinweise und Dateistatus prüfen."}</p>
      <p className="detail-hint">Beobachtungszeitraum: {date.format(report.startedAtMs)} – {date.format(report.finishedAtMs)}. {report.processed.toLocaleString("de-DE")} / {report.total.toLocaleString("de-DE")} Dateien bearbeitet · {bytes(report.bytesRead)} gelesen, einschließlich Bytevergleich. Quellen wurden nicht eingefroren.</p>
      {report.complete && !report.cancelled && report.groups.length === 0 && <p>Keine byte-identischen Dateien im vollständig geprüften Umfang gefunden.</p>}
      {report.groups.length > 0 && <>
        <h3>Byte-identische Dateigruppen</h3>
        <ul className="duplicate-groups">{report.groups.slice(groupOffset, groupOffset + 5).map((item, index) => <li key={groupOffset + index}>
          <button className="button outlined" aria-label={`Gruppe ${groupOffset + index + 1} anzeigen`}
            aria-pressed={groupIndex === groupOffset + index}
            onClick={() => { setGroupIndex(groupOffset + index); setMemberOffset(0); }}>
            Gruppe {groupOffset + index + 1} · {item.members.length} Dateien · {bytes(item.sizeBytes)} je Datei
          </button>
        </li>)}</ul>
        {pagination("Gruppen", groupOffset, report.groups.length, 5, (next) => { setGroupOffset(next); setGroupIndex(next); setMemberOffset(0); })}
        {group && <section aria-label="Gruppenmitglieder">
          <h4>Gruppe {groupIndex + 1}</h4>
          <p className="duplicate-hash">SHA-256: <code>{group.sha256}</code> · {bytes(group.sizeBytes)} je Datei</p>
          {group.sharedSource && <p className="detail-hint">Gemeinsame Dateiidentität (z. B. Hardlinks) in dieser Gruppe; keine Aussage über freigebbaren Speicher.</p>}
          <div className="duplicate-table-scroll"><table className="sound-table">
            <thead><tr><th scope="col">Datei und Variante</th><th scope="col">Paket und Originalpfad</th></tr></thead>
            <tbody>{group.members.slice(memberOffset, memberOffset + 10).map((id) => {
              const file = filesById.get(id);
              return <tr key={id}>
                <td>{file ? <>{file.originalFilename} · {file.format}<br />{identity(file)}</> : `Datei #${id} fehlt im Bericht`}</td>
                <td>{file && <>{file.packageName}<br />{file.relativePath}</>}</td>
              </tr>;
            })}</tbody>
          </table></div>
          {pagination("Mitglieder", memberOffset, group.members.length, 10, setMemberOffset)}
        </section>}
      </>}
      <section aria-label="Dateiprüfung">
        <h3>Dateiprüfung ({report.files.length.toLocaleString("de-DE")})</h3>
        <div className="duplicate-table-scroll"><table className="sound-table">
          <thead><tr><th scope="col">Datei und Herkunft</th><th scope="col">Status und Grund</th></tr></thead>
          <tbody>{report.files.slice(fileOffset, fileOffset + 20).map((file) => <tr key={file.fileId}>
            <td>{file.originalFilename} · {file.format}<br />{identity(file)}<br />{file.packageName} · {file.relativePath} · {bytes(file.sizeBytes)}</td>
            <td>{fileLabels[file.status]}{file.reason && <p>{file.reason}</p>}</td>
          </tr>)}</tbody>
        </table></div>
        {pagination("Dateien", fileOffset, report.files.length, 20, setFileOffset)}
      </section>
      {report.issues.length > 0 && <section aria-label="Prüfhinweise">
        <h3>Hinweise ({report.issues.length})</h3>
        <ul className="duplicate-issues">{report.issues.slice(issueOffset, issueOffset + 10).map((issue, index) => <li key={issueOffset + index}>{issue}</li>)}</ul>
        {pagination("Hinweise", issueOffset, report.issues.length, 10, setIssueOffset)}
      </section>}
    </>}
    <p className="detail-hint">Gleiche Dateibytes beweisen keine identischen Begleitdateien oder Lizenzen. Es wird nichts zusammengeführt oder gelöscht.</p>
  </dialog>;
}
