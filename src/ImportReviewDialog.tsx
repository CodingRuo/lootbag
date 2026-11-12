import { useEffect, useRef, useState } from "react";

export type PreparedImport = {
  requestId: string;
  source: { path: string; isZip: boolean };
  binding: "verified" | "unverified";
  zipEstimate: null;
  evidence: {
    status: "complete" | "incomplete" | "unavailable";
    incomingFiles: number | null;
    candidates: number | null;
    examinedCandidates: number;
    duplicateIncomingFiles: number;
    unavailableCandidates: number;
    existingVariants: number | null;
    unavailableExistingVariants: number;
    matches: { incomingRelativePath: string; sizeBytes: number; existingFileId: number; existingAssetId: number; existingPackageId: number; existingPackageName: string; existingOriginalFilename: string; existingRelativePath: string; sha256: string }[];
    issues: string[];
    detailsTruncated: boolean;
  };
};
export type PreparationProgress = {
  stage: "inventory" | "compare" | "verify";
  examinedCandidates: number;
  totalCandidates: number | null;
  bytesRead: number;
};

export function ImportReviewDialog({ prepared, remaining, busy, error, onProceed, onCancel, formatBytes: bytes }: {
  prepared: PreparedImport;
  remaining: number;
  busy: boolean;
  error: string | null;
  onProceed: () => void;
  onCancel: () => void;
  formatBytes: (value: number) => string;
}) {
  const dialog = useRef<HTMLDialogElement>(null);
  const [matchOffset, setMatchOffset] = useState(0);
  const [issueOffset, setIssueOffset] = useState(0);
  useEffect(() => { dialog.current?.showModal(); }, []);
  const evidence = prepared.evidence;
  const count = (value: number | null) => value === null ? "Nicht ermittelt" : value.toLocaleString("de-DE");
  function page(name: string, offset: number, total: number, change: (value: number) => void) {
    return <nav className="trash-pagination" aria-label={`${name} blättern`}><span>{name}: {total ? offset + 1 : 0}–{Math.min(offset + 10, total)} von {total}</span><button className="button outlined" aria-label={`${name}: Zurück`} disabled={offset === 0} onClick={() => change(Math.max(0, offset - 10))}>Zurück</button><button className="button outlined" aria-label={`${name}: Weiter`} disabled={offset + 10 >= total} onClick={() => change(offset + 10)}>Weiter</button></nav>;
  }
  return <dialog ref={dialog} className="trash-dialog import-review-dialog" aria-labelledby="import-review-title"
    onCancel={(event) => { event.preventDefault(); if (!busy) onCancel(); }} onClose={() => { if (!busy) onCancel(); }}>
    <div className="trash-heading"><h2 id="import-review-title">Paket vor dem Import prüfen</h2></div>
    <p className="source-path">{prepared.source.path} · {prepared.source.isZip ? "ZIP-Paket" : "Quellordner"}</p>
    <p role="status" className={evidence.status === "complete" ? "detail-hint" : "inline-error"}>{evidence.status === "complete" ? "Duplikatprüfung vollständig im geprüften Umfang." : evidence.status === "unavailable" ? "Duplikatprüfung nicht verfügbar. Keine Aussage über fehlende Duplikate möglich." : "Duplikatprüfung unvollständig. Ungeprüfte Dateien sind keine fehlenden Duplikate."}</p>
    <p className={prepared.binding === "verified" ? "detail-hint" : "inline-error"}>{prepared.binding === "verified" ? "Kopieren ist an den geprüften Paketinhalt gebunden." : "Unverifizierter Ganzpaket-Import: Paketinhalt und Begleitdateien sind nicht vollständig an diese Prüfung gebunden."}</p>
    <p className="detail-hint">Paketdateien: {count(evidence.incomingFiles)} · Asset-Dateikandidaten: {count(evidence.candidates)} · Untersucht: {count(evidence.examinedCandidates)} · Eingehende Dateien mit Duplikaten: {count(evidence.duplicateIncomingFiles)}</p>
    <p className="detail-hint">Bestehende Dateivarianten: {count(evidence.existingVariants)} · Nicht verfügbare eingehende Kandidaten: {count(evidence.unavailableCandidates)} · Nicht verfügbare bestehende Varianten: {count(evidence.unavailableExistingVariants)}</p>
    {evidence.matches.length > 0 && <section aria-label="Duplikatbelege"><h3>Byte-identische Dateien</h3><div className="import-review-scroll"><table className="sound-table"><thead><tr><th scope="col">Eingehende Datei</th><th scope="col">Bestehende Variante</th></tr></thead><tbody>{evidence.matches.slice(matchOffset, matchOffset + 10).map((match, index) => <tr key={matchOffset + index}><td>{match.incomingRelativePath}<br />{bytes(match.sizeBytes)}<br /><code>{match.sha256}</code></td><td>{match.existingPackageName}<br />{match.existingOriginalFilename}<br />{match.existingRelativePath}<br />Datei #{match.existingFileId} · Asset #{match.existingAssetId} · Paket #{match.existingPackageId}</td></tr>)}</tbody></table></div>{page("Belege", matchOffset, evidence.matches.length, setMatchOffset)}</section>}
    {evidence.issues.length > 0 && <section aria-label="Importhinweise"><h3>Hinweise</h3><ul>{evidence.issues.slice(issueOffset, issueOffset + 10).map((issue, index) => <li key={issueOffset + index}>{issue}</li>)}</ul>{page("Hinweise", issueOffset, evidence.issues.length, setIssueOffset)}</section>}
    {evidence.detailsTruncated && <p className="detail-hint">Weitere Belege oder Hinweise wurden nicht einzeln aufgelistet. Die Prüfung kann mehr Treffer als diese Detailseiten enthalten.</p>}
    <p className="detail-hint">Es wird die gesamte Ordnerstruktur mit allen Begleitdateien importiert. Gleiche Dateibytes beweisen keine identischen Modell-Abhängigkeiten oder Lizenzen. Es wird nichts zusammengeführt, ausgelassen oder gelöscht.</p>
    {remaining > 0 && <p className="detail-hint">Abbrechen überspringt dieses ganze Paket. Danach werden {remaining} weitere abgelegte Quellen einzeln geprüft.</p>}
    {error && <p className="inline-error" role="alert">{error}</p>}
    <div className="import-review-actions"><button className="button primary" disabled={busy} onClick={onProceed}>Gesamtes Paket trotzdem importieren</button><button className="button outlined" disabled={busy} onClick={onCancel}>{busy ? "Abbruch angefordert …" : "Abbrechen"}</button></div>
  </dialog>;
}
