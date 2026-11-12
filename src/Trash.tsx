import { useEffect, useId, useRef, useState, type ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";

export type TrashEntry = {
  id: number;
  packageId: number;
  fileId: number | null;
  name: string;
  packageName: string;
  deletedAt: number;
  originalFolder: string | null;
  parentTrashed: boolean;
};

export function TrashActions({ children, label, disabled, onTrash, className = "" }: {
  children: ReactNode;
  label: string;
  disabled: boolean;
  onTrash: () => void;
  className?: string;
}) {
  const id = useId();
  const menu = useRef<HTMLDivElement>(null);
  const button = useRef<HTMLButtonElement>(null);

  function position(x: number, y: number) {
    if (!menu.current) return;
    menu.current.style.left = `${Math.max(8, Math.min(x, window.innerWidth - 232))}px`;
    menu.current.style.top = `${Math.max(8, Math.min(y, window.innerHeight - 64))}px`;
  }

  return (
    <div className={`trash-actions ${className}`} onContextMenu={(event) => event.preventDefault()} onMouseUp={(event) => {
      if (event.button !== 2 || disabled) return;
      // WebKit fires contextmenu before pointerup light-dismiss; open after release.
      position(event.clientX, event.clientY);
      menu.current?.showPopover();
    }}>
      {children}
      <button ref={button} className="icon-button" aria-label={`${label}: Aktionen`} aria-haspopup="menu"
        disabled={disabled} popoverTarget={id} onClick={(event) => {
          const rect = event.currentTarget.getBoundingClientRect();
          position(rect.right - 224, rect.bottom + 4);
        }}>⋯</button>
      <div ref={menu} id={id} popover="auto" className="trash-menu" role="menu"
        onToggle={(event) => {
          if (event.currentTarget.matches(":popover-open")) event.currentTarget.querySelector("button")?.focus();
        }} onKeyDown={(event) => {
          if (event.key === "Escape") {
            event.preventDefault();
            menu.current?.hidePopover();
            button.current?.focus();
          }
        }}>
        <button role="menuitem" disabled={disabled} onClick={() => {
          menu.current?.hidePopover();
          button.current?.focus();
          onTrash();
        }}>In Papierkorb verschieben</button>
      </div>
    </div>
  );
}

const PAGE_SIZE = 40;
const date = new Intl.DateTimeFormat("de-DE", { dateStyle: "short", timeStyle: "short" });

export function TrashDialog({ root, revision, disabled, notice, onClose, onRestore, onPrepareRemoval }: {
  root: string;
  revision: number;
  disabled: boolean;
  notice: { tone: "error" | "success"; message: string } | null;
  onClose: () => void;
  onRestore: (entry: TrashEntry) => void;
  onPrepareRemoval: (entry: TrashEntry) => void;
}) {
  const dialog = useRef<HTMLDialogElement>(null);
  const [entries, setEntries] = useState<TrashEntry[]>([]);
  const [offset, setOffset] = useState(0);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    dialog.current?.showModal();
  }, []);

  useEffect(() => {
    let active = true;
    setLoading(true);
    setError(null);
    // ponytail: load trash once per catalog change; paginate the public API if this list becomes large.
    invoke<TrashEntry[]>("trash_entries").then((result) => {
      if (!active) return;
      setEntries(result);
      setOffset((value) => value >= result.length ? 0 : value);
    }).catch((error: unknown) => {
      if (active) setError(typeof error === "string" ? error : "Papierkorb konnte nicht geladen werden.");
    }).finally(() => {
      if (active) setLoading(false);
    });
    return () => { active = false; };
  }, [root, revision]);

  return (
    <dialog ref={dialog} className="trash-dialog" aria-labelledby="trash-title" onCancel={onClose} onClose={onClose}>
      <div className="trash-heading">
        <h2 id="trash-title">Interner Papierkorb</h2>
        <button className="button outlined" onClick={onClose}>Schließen</button>
      </div>
      <p className="detail-hint">Gelöschte Katalogeinträge können wiederhergestellt werden. Importierte Dateien bleiben gespeichert und belegen weiterhin Speicherplatz. Externe Originale und Quellarchive bleiben erhalten. Nur externe Pakete können nach Vorbereitung und Bestätigung endgültig aus dem Katalog entfernt werden; dabei werden 0 Byte Dateien entfernt.</p>
      {notice && <p role={notice.tone === "error" ? "alert" : "status"} className={notice.tone === "error" ? "inline-error" : "detail-hint"}>{notice.message}</p>}
      {error ? <p role="alert" className="inline-error">{error}</p> : loading && !entries.length
        ? <p role="status">Papierkorb wird geladen …</p> : entries.length ? <>
          <div className="trash-scroll" aria-busy={loading}>
            <table className="sound-table">
              <thead><tr><th scope="col">Eintrag</th><th scope="col">Gelöscht am</th><th scope="col">Aktion</th></tr></thead>
              <tbody>{entries.slice(offset, offset + PAGE_SIZE).map((entry) => <tr key={entry.id}>
                <td>
                  <strong>{entry.name}</strong>
                  <span className="table-folder">{entry.fileId === null ? "Paket" : `Datei · ${entry.packageName} · ${entry.originalFolder || "Paketwurzel"}`}</span>
                  {entry.parentTrashed && <span className="table-folder">Zuerst das Paket wiederherstellen.</span>}
                </td>
                <td>{date.format(new Date(entry.deletedAt * 1000))}</td>
                <td><div className="trash-row-actions"><button className="button outlined" disabled={disabled || loading || entry.parentTrashed} onClick={() => onRestore(entry)}
                  aria-label={`${entry.name} wiederherstellen`}>Wiederherstellen</button>
                  {entry.fileId === null && <button className="button outlined" disabled={disabled || loading}
                    onClick={() => onPrepareRemoval(entry)} aria-label={`${entry.name}: Entfernen vorbereiten`}>Entfernen vorbereiten</button>}
                </div></td>
              </tr>)}</tbody>
            </table>
          </div>
          <footer className="trash-pagination">
            <span>{offset + 1}–{Math.min(offset + PAGE_SIZE, entries.length)} von {entries.length} Einträgen</span>
            <button className="button outlined" disabled={loading || offset === 0} onClick={() => setOffset(Math.max(0, offset - PAGE_SIZE))}>Vorherige Seite</button>
            <button className="button outlined" disabled={loading || offset + PAGE_SIZE >= entries.length} onClick={() => setOffset(offset + PAGE_SIZE)}>Nächste Seite</button>
          </footer>
        </> : <p>Der Papierkorb ist leer.</p>}
    </dialog>
  );
}
