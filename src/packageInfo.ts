// Package record (shop, licence, author …): field labels, saving and the inspector summary (testable without Tauri).
export type PackageInfo = {
  store: string | null;
  url: string | null;
  license: string | null;
  author: string | null;
  purchased: string | null;
  version: string | null;
  notes: string | null;
};
export type LicenseSuggestion = { license: string; file: string; excerpt: string };
export type PackageInfoDetails = PackageInfo & { licenseFiles: string[]; licenseSuggestions: LicenseSuggestion[] };
type Invoke = <T>(command: string, args?: Record<string, unknown>) => Promise<T>;

export const PACKAGE_INFO_FIELDS: { key: keyof PackageInfo; label: string; max: number }[] = [
  { key: "store", label: "Shop", max: 200 },
  { key: "url", label: "Link", max: 2048 },
  { key: "license", label: "Lizenz", max: 200 },
  { key: "author", label: "Urheber", max: 200 },
  { key: "purchased", label: "Gekauft am", max: 10 },
  { key: "version", label: "Version", max: 200 },
  { key: "notes", label: "Notizen", max: 2000 },
];

// Empty inputs are saved as null; the backend trims and validates.
export function savePackageInfo(invoke: Invoke, packageId: number, form: Record<keyof PackageInfo, string>) {
  const info = Object.fromEntries(PACKAGE_INFO_FIELDS.map(({ key }) => [key, form[key].trim() || null]));
  return invoke<void>("set_package_info", { packageId, info });
}

// Facts shown in the inspector's origin tab; empty means "Keine Lizenz hinterlegt" when no licence is set either.
export function originFacts(info: PackageInfo | undefined) {
  return PACKAGE_INFO_FIELDS.filter(({ key }) => ["store", "license", "author", "url"].includes(key) && info?.[key])
    .map(({ key, label }) => ({ key, label, value: info![key]! }));
}

// "Übernehmen" fills the licence only while it is empty, judged on the latest form (the user may have typed meanwhile).
export function adoptLicense<T extends { license: string }>(form: T, license: string): T {
  return form.license.trim() ? form : { ...form, license };
}
