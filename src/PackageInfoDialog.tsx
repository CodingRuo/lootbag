import { useEffect, useRef, useState, type FormEvent } from "react";
import { invoke } from "@tauri-apps/api/core";
import { PACKAGE_INFO_FIELDS, type PackageInfo, type PackageInfoDetails } from "./packageInfo.ts";
import type { AssetPackage, Notice } from "./ui.tsx";

// "Paketinfo bearbeiten…": shop, link, licence, author, purchase date, version and notes of one package.
export function PackageInfoDialog({ pkg, disabled, notice, onSave, onClose }: {
  pkg: AssetPackage;
  disabled: boolean;
  notice: Notice | null;
  onSave: (form: Record<keyof PackageInfo, string>) => void;
  onClose: () => void;
}) {
  const dialog = useRef<HTMLDialogElement>(null);
  const [form, setForm] = useState(() => Object.fromEntries(PACKAGE_INFO_FIELDS.map(({ key }) => [key, pkg.info[key] ?? ""])) as Record<keyof PackageInfo, string>);
  const [licenseFiles, setLicenseFiles] = useState<string[]>([]);
  useEffect(() => { dialog.current?.showModal(); }, []);
  useEffect(() => {
    let active = true;
    invoke<PackageInfoDetails>("get_package_info", { packageId: pkg.id })
      .then((details) => { if (active) setLicenseFiles(details.licenseFiles); })
      .catch(() => {});
    return () => { active = false; };
  }, [pkg.id]);

  return (
    <dialog ref={dialog} className="trash-dialog folder-dialog" aria-labelledby="package-info-title" onCancel={onClose} onClose={onClose}>
      <div className="trash-heading">
        <h2 id="package-info-title">Paketinfo</h2>
        <button className="button outlined" onClick={onClose}>Schließen</button>
      </div>
      <p className="detail-hint package-info-name">{pkg.name}</p>
      <form className="annotation-editor" onSubmit={(event: FormEvent<HTMLFormElement>) => {
        event.preventDefault();
        onSave(form);
      }}>
        {PACKAGE_INFO_FIELDS.map(({ key, label, max }) => {
          const props = { value: form[key], maxLength: max, disabled, onChange: (event: { target: { value: string } }) => setForm({ ...form, [key]: event.target.value }) };
          return <label key={key}>
            {label}
            {key === "notes" ? <textarea rows={3} {...props} />
              : <input {...props} type={key === "purchased" ? "date" : "text"} inputMode={key === "url" ? "url" : undefined}
                placeholder={key === "store" ? "z. B. itch.io, Synty Store, Kenney" : key === "license" ? "z. B. CC0, Synty Standard License" : key === "url" ? "https://…" : undefined} />}
          </label>;
        })}
        {licenseFiles.length > 0 && <p className="detail-hint">Im Paket gefunden: {licenseFiles.join(", ")}</p>}
        {notice?.tone === "error" && <p className="inline-error" role="alert">{notice.message}</p>}
        <button className="button primary" type="submit" disabled={disabled}>Speichern</button>
      </form>
    </dialog>
  );
}
