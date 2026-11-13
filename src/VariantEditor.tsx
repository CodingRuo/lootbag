import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { AssetKind } from "./ui.tsx";

type TargetAsset = {
  id: number;
  name: string;
  files: { originalFolder: string; format: string }[];
};

export default function VariantEditor({
  assetId, packageId, kind, fileCount, disabled, onSplit, onAssign,
}: {
  assetId: number;
  packageId: number;
  kind: AssetKind;
  fileCount: number;
  disabled: boolean;
  onSplit: () => void;
  onAssign: (targetAssetId: number) => void;
}) {
  const [expanded, setExpanded] = useState(false);
  const [text, setText] = useState("");
  const [targets, setTargets] = useState<TargetAsset[]>([]);
  const [total, setTotal] = useState(0);
  const [targetId, setTargetId] = useState<number | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!expanded) return;
    let active = true;
    setLoading(true);
    setError(null);
    const timer = window.setTimeout(() => {
      invoke<{ items: TargetAsset[]; total: number }>("query_catalog", {
        query: { text, packageId, kind, originalFolder: null, format: null, tag: null, offset: 0, limit: 20 },
      }).then((page) => {
        if (!active) return;
        setTargets(page.items.filter((item) => item.id !== assetId));
        setTotal(page.total);
      }).catch((error: unknown) => {
        if (active) setError(typeof error === "string" ? error : "Ziel-Assets konnten nicht geladen werden.");
      }).finally(() => {
        if (active) setLoading(false);
      });
    }, 160);
    return () => {
      active = false;
      window.clearTimeout(timer);
    };
  }, [expanded, text, assetId, packageId, kind]);

  return (
    <details className="variant-editor" onToggle={(event) => setExpanded(event.currentTarget.open)}>
      <summary>Variantenzuordnung korrigieren</summary>
      <p className="detail-hint">Name, Tags und Ordner werden übernommen.</p>
      <button className="button outlined" disabled={disabled || fileCount < 2} onClick={onSplit}>
        Als eigenes Asset abspalten
      </button>
      <label>
        Ziel-Asset derselben Kategorie im Paket suchen
        <input type="search" value={text} disabled={disabled} onChange={(event) => {
          setText(event.target.value);
          setTargetId(null);
        }} placeholder="Name, Originaldatei oder Ordner …" />
      </label>
      <label>
        Datei einem Asset zuordnen
        <select value={targetId ?? ""} disabled={disabled || loading || error !== null} onChange={(event) => setTargetId(event.target.value ? Number(event.target.value) : null)}>
          <option value="">Asset auswählen</option>
          {targets.map((item) => (
            <option key={item.id} value={item.id}>
              {item.name} · {Array.from(new Set(item.files.map((file) => file.originalFolder || "Paketwurzel"))).join(", ")} · {item.files.map((file) => file.format).join(", ")}
            </option>
          ))}
        </select>
      </label>
      {loading ? <p className="detail-hint" role="status">Ziel-Assets werden gesucht …</p> : total > 20 ? <p className="detail-hint">Die ersten 20 Treffer. Grenze die Suche für weitere Assets ein.</p> : !targets.length && <p className="detail-hint">Kein anderes passendes Asset gefunden.</p>}
      {error && <p className="inline-error" role="alert">{error}</p>}
      <button className="button outlined" disabled={disabled || loading || error !== null || targetId === null} onClick={() => { if (targetId !== null) onAssign(targetId); }}>
        Ausgewählte Datei zuordnen
      </button>
      <p className="detail-hint">Tags und Ordner werden zusammengeführt.</p>
    </details>
  );
}
