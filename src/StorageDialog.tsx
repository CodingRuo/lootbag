import { useEffect, useRef, useState } from "react";
import { Channel, invoke } from "@tauri-apps/api/core";

export type StorageProgress = { stage: "walk" | "verify" | "finished"; visitedEntries: number };
export type StorageReport = {
  complete: boolean;
  cancelled: boolean;
  startedAtMs: number;
  finishedAtMs: number;
  totalBytes: number;
  categories: { category: string; bytes: number; files: number; paths: number }[];
  visitedEntries: number;
  excludedEntries: number;
  issueCount: number;
  issues: string[];
  external: null | { recordedBytes: number; recordedVersions: number; references: number; activeVersions: number; trashVersions: number; sharedVersions: number; unqualifiedReferences: number };
};
const labels: Record<string, string> = { managed: "Verwaltete Pakete und Begleitdateien", trash: "Im Papierkorb behalten", support: "Katalog, Seiten- und Katalog-Reparaturdateien", staging: "Import-Zwischendateien", unclassified: "Weitere Dateien", shared: "Hardlinks über mehrere Kategorien" };
const date = new Intl.DateTimeFormat("de-DE", { dateStyle: "short", timeStyle: "medium" });

export function StorageDialog({ root, onClose, formatBytes: bytes }: { root: string; onClose: () => void; formatBytes: (value: number) => string }) {
  const dialog = useRef<HTMLDialogElement>(null);
  const request = useRef<{ id: string; root: string } | null>(null);
  const [report, setReport] = useState<StorageReport | null>(null);
  const [progress, setProgress] = useState<StorageProgress | null>(null);
  const [running, setRunning] = useState(false);
  const [stopping, setStopping] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    dialog.current?.showModal();
  }, []);
  useEffect(() => {
    setReport(null); setProgress(null); setError(null); setRunning(false); setStopping(false);
    return () => {
      const older = request.current;
      request.current = null;
      if (older) void invoke("cancel_storage_report", { requestId: older.id }).catch(() => {});
    };
  }, [root]);

  function close() {
    const older = request.current;
    request.current = null;
    if (older) void invoke("cancel_storage_report", { requestId: older.id }).catch(() => {});
    onClose();
  }
  async function refresh() {
    const job = { id: crypto.randomUUID(), root };
    request.current = job;
    setRunning(true); setStopping(false); setError(null); setReport(null);
    setProgress({ stage: "walk", visitedEntries: 0 });
    const onProgress = new Channel<StorageProgress>();
    onProgress.onmessage = (value) => { if (request.current === job) setProgress(value); };
    try {
      const result = await invoke<StorageReport>("storage_report", { root: job.root, requestId: job.id, onProgress });
      if (request.current === job) setReport(result);
    } catch (error: unknown) {
      if (request.current === job) setError(typeof error === "string" ? error : "Speichermessung konnte nicht abgeschlossen werden.");
    } finally {
      if (request.current === job) { request.current = null; setRunning(false); setStopping(false); }
    }
  }
  async function stop() {
    const job = request.current;
    if (!job) return;
    setStopping(true);
    try { await invoke("cancel_storage_report", { requestId: job.id }); }
    catch (error: unknown) {
      if (request.current === job) { setStopping(false); setError(typeof error === "string" ? error : "Abbruch konnte nicht angefordert werden."); }
    }
  }

  return <dialog ref={dialog} className="trash-dialog storage-dialog" aria-labelledby="storage-title" onCancel={close} onClose={close}>
    <div className="trash-heading"><h2 id="storage-title">Speicherübersicht</h2><button className="button outlined" onClick={close}>Schließen</button></div>
    <p className="detail-hint">Misst eindeutige logische Dateibytes in dieser Bibliothek. Sparse-Dateien und APFS-Kopien können auf dem Laufwerk anders belegt sein. Dies ist keine Schätzung des freigebbaren Speicherplatzes.</p>
    <p className="source-path">{root}</p>
    <div className="storage-report-actions"><button className="button outlined" disabled={running} onClick={() => void refresh()}>Aktualisieren</button>
      {running && <button className="button outlined" disabled={stopping} onClick={() => void stop()}>{stopping ? "Abbruch angefordert …" : "Messung abbrechen"}</button>}</div>
    {running && <p role="status">{stopping ? "Messung wird abgebrochen …" : progress?.stage === "verify" ? "Dateien und Zuordnung werden überprüft …" : "Bibliothek wird gemessen …"} {progress?.visitedEntries.toLocaleString("de-DE")} Einträge geprüft.</p>}
    {error && <p role="alert" className="inline-error">{error}</p>}
    {report && <>
      <p role="status" className={report.complete ? "detail-hint" : "inline-error"}>{report.complete ? "Messung vollständig innerhalb des geprüften Bereichs." : report.cancelled ? "Messung abgebrochen. Die angezeigten Werte sind unvollständig." : "Messung unvollständig. Bitte Hinweise prüfen und erneut aktualisieren."}</p>
      <p className="detail-hint">Messzeitraum: {date.format(report.startedAtMs)} – {date.format(report.finishedAtMs)}. Das Dateisystem wurde währenddessen nicht eingefroren.</p>
      <table className="sound-table"><thead><tr><th scope="col">Kategorie</th><th scope="col">Logische Bytes</th><th scope="col">Eindeutige Dateien</th><th scope="col">Dateinamen</th></tr></thead>
        <tbody>{report.categories.map((item) => <tr key={item.category}><td>{labels[item.category] ?? item.category}</td><td>{bytes(item.bytes)}</td><td>{item.files.toLocaleString("de-DE")}</td><td>{item.paths.toLocaleString("de-DE")}</td></tr>)}</tbody>
        <tfoot><tr><th scope="row">{report.complete ? "Gesamt" : "Bisher gemessen"}</th><td>{bytes(report.totalBytes)}</td><td colSpan={2}>{report.visitedEntries.toLocaleString("de-DE")} Einträge geprüft</td></tr></tfoot>
      </table>
      <p className="detail-hint">Hardlinks werden einmal gezählt. Der Papierkorb behält Dateien; Projektordner erzeugen keine Kopien. Import-Zwischendateien sind nicht automatisch überflüssig. Katalog-Reparaturdateien sind keine vollständige Bibliothekssicherung.</p>
      {report.issueCount > 0 && <div><h3>Hinweise ({report.issueCount.toLocaleString("de-DE")})</h3><p className="detail-hint">{report.excludedEntries.toLocaleString("de-DE")} Einträge ausgelassen.</p><ul className="storage-issues">{report.issues.map((issue, index) => <li key={index}>{issue}</li>)}</ul>{report.issueCount > report.issues.length && <p className="detail-hint">Weitere Hinweise wurden aus Platzgründen nicht einzeln aufgelistet.</p>}</div>}
      {report.external ? <><h3>Externe Quellen – historische Katalogwerte</h3>
      <p>{bytes(report.external.recordedBytes)} in {report.external.recordedVersions.toLocaleString("de-DE")} aufgezeichneten Dateiversionen aus {report.external.references.toLocaleString("de-DE")} Referenzen.</p>
      <p className="detail-hint">{report.external.activeVersions.toLocaleString("de-DE")} aktiv · {report.external.trashVersions.toLocaleString("de-DE")} nur Papierkorb · {report.external.sharedVersions.toLocaleString("de-DE")} in beiden. Gleiche qualifizierte Identitäts- und Änderungsversionen werden zusammengefasst.</p>
      {report.external.unqualifiedReferences > 0 && <p className="detail-hint">{report.external.unqualifiedReferences.toLocaleString("de-DE")} Referenzen mit ungeprüfter Identität wurden getrennt gezählt.</p>}
      <p className="detail-hint">Diese Werte gehören nicht zur Bibliothekssumme. Sie stammen vom letzten Einbinden, können veraltet oder offline sein und enthalten keine nicht katalogisierten Begleitdateien. Externe Ordner wurden nicht durchsucht.</p></> : <p className="detail-hint">Externe Zuordnung wurde noch nicht vollständig erfasst.</p>}
    </>}
    <p className="detail-hint">App-eigene dauerhafte Vorschaudateien: In dieser Bibliothek ist kein Vorschaudatei-Cache eingerichtet. Laufzeitvorschauen verwenden Arbeitsspeicher; RAM und Betriebssystem-/WebView-Caches außerhalb der Bibliothek werden hier nicht gemessen.</p>
  </dialog>;
}
