import { useEffect, useRef, useState, type FormEvent } from "react";

export type ProjectFolder = {
  id: number;
  name: string;
  parentId: number | null;
  assetCount: number;
};

export function folderPath(id: number, folders: ProjectFolder[]) {
  const names: string[] = [];
  // ponytail: small project folder list; index parent IDs if this grows to thousands.
  let folder = folders.find((item) => item.id === id);
  while (folder) {
    names.unshift(folder.name);
    folder = folders.find((item) => item.id === folder?.parentId);
  }
  return names.join(" / ");
}

export function FolderDialog({ folders, parentId, disabled, notice, onCreate, onClose }: {
  folders: ProjectFolder[];
  parentId: number | null;
  disabled: boolean;
  notice: { tone: "error" | "success"; message: string } | null;
  onCreate: (name: string, parentId: number | null) => void;
  onClose: () => void;
}) {
  const dialog = useRef<HTMLDialogElement>(null);
  const [name, setName] = useState("");
  const [parent, setParent] = useState(parentId);
  useEffect(() => { dialog.current?.showModal(); }, []);

  return (
    <dialog ref={dialog} className="trash-dialog folder-dialog" aria-labelledby="folder-dialog-title" onCancel={onClose} onClose={onClose}>
      <div className="trash-heading">
        <h2 id="folder-dialog-title">Projektordner anlegen</h2>
        <button className="button outlined" onClick={onClose}>Schließen</button>
      </div>
      <form className="annotation-editor" onSubmit={(event: FormEvent<HTMLFormElement>) => {
        event.preventDefault();
        onCreate(name, parent);
      }}>
        <label>
          Ordnername
          <input value={name} onChange={(event) => setName(event.target.value)} required maxLength={256} disabled={disabled} autoFocus />
        </label>
        <label>
          Übergeordneter Ordner
          <select value={parent ?? ""} disabled={disabled} onChange={(event) => setParent(event.target.value ? Number(event.target.value) : null)}>
            <option value="">Keiner · Projektwurzel</option>
            {folders.map((folder) => <option key={folder.id} value={folder.id}>{folderPath(folder.id, folders)}</option>)}
          </select>
        </label>
        <p className="detail-hint">Projektordner sammeln vorhandene Assets als Verknüpfungen.</p>
        {notice?.tone === "error" && <p className="inline-error" role="alert">{notice.message}</p>}
        <button className="button primary" type="submit" disabled={disabled || !name.trim()}>Ordner anlegen</button>
      </form>
    </dialog>
  );
}

export function FolderMembershipEditor({ folders, folderIds, disabled, onAdd, onRemove }: {
  folders: ProjectFolder[];
  folderIds: number[];
  disabled: boolean;
  onAdd: (folderId: number) => void;
  onRemove: (folderId: number) => void;
}) {
  const [folderId, setFolderId] = useState<number | null>(null);
  const available = folders.filter((folder) => !folderIds.includes(folder.id));
  return (
    <section className="annotations folder-memberships" aria-labelledby="project-memberships-title">
      <h3 id="project-memberships-title">Projektordner</h3>
      {folderIds.length > 0 && <ul>{folders.filter((folder) => folderIds.includes(folder.id)).map((folder) => {
        const path = folderPath(folder.id, folders);
        return <li key={folder.id}>
          <span>{path}</span>
          <button className="text-button" disabled={disabled} aria-label={`Aus ${path} entfernen`} onClick={() => onRemove(folder.id)}>Entfernen</button>
        </li>;
      })}</ul>}
      {available.length > 0 ? <form className="annotation-editor" onSubmit={(event) => {
        event.preventDefault();
        if (folderId !== null) { onAdd(folderId); setFolderId(null); }
      }}>
        <label>
          Zum Projektordner hinzufügen
          <select value={folderId ?? ""} disabled={disabled} onChange={(event) => setFolderId(event.target.value ? Number(event.target.value) : null)}>
            <option value="">Ordner auswählen</option>
            {available.map((folder) => <option key={folder.id} value={folder.id}>{folderPath(folder.id, folders)}</option>)}
          </select>
        </label>
        <button className="button outlined" type="submit" disabled={disabled || folderId === null}>Hinzufügen</button>
      </form> : !folders.length && <p className="detail-hint">Lege zuerst einen Projektordner in der Seitenleiste an.</p>}
      <p className="detail-hint">Entfernen löst nur die Verknüpfung.</p>
    </section>
  );
}
