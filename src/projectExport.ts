// Project folder selections: per-member variant/role saving and the export flow (testable without Tauri).
export type FolderMember = { folderId: number; fileId: number | null; role: string | null };
export type ProjectExportPlan = { project: string; assetCount: number; sizeBytes: number; missing: string[]; substitutions: number };
type Invoke = <T>(command: string, args?: Record<string, unknown>) => Promise<T>;

export function saveFolderMember(invoke: Invoke, assetId: number, member: FolderMember) {
  return invoke<void>("set_folder_member", { folderId: member.folderId, assetId, fileId: member.fileId, role: member.role });
}

// Role shown in the catalog for the active folder filter, which includes subfolders.
export function memberRole(members: FolderMember[], folderId: number, folders: { id: number; parentId: number | null }[]) {
  const inside = (id: number | null): boolean => id === folderId || (id !== null && inside(folders.find((folder) => folder.id === id)?.parentId ?? null));
  return members.find((member) => member.folderId === folderId)?.role ?? members.find((member) => member.role && inside(member.folderId))?.role ?? null;
}

export function missingMessage(plan: ProjectExportPlan) {
  const shown = plan.missing.slice(0, 5).map((item) => `• ${item}`).join("\n");
  const more = plan.missing.length > 5 ? `\n… und ${plan.missing.length - 5} weitere` : "";
  const missing = plan.missing.length ? `${plan.missing.length} von ${plan.assetCount} Assets sind nicht verfügbar und werden ohne Datei im Manifest vermerkt:\n${shown}${more}\n\n` : "";
  const substituted = plan.substitutions ? `${plan.substitutions} Texturen werden durch gleichnamige Dateien ersetzt; die Modelle verweisen weiter auf das Original (siehe Manifest).\n\n` : "";
  return `${missing}${substituted}Trotzdem exportieren?`;
}

// Picks a destination, reports unavailable files and texture substitutions before copying and lets the user continue or cancel.
export async function exportProject(folderId: number, invoke: Invoke, pickDirectory: () => Promise<string | null>, confirmMissing: (message: string) => Promise<boolean>) {
  const parent = await pickDirectory();
  if (!parent) return null;
  const plan = await invoke<ProjectExportPlan>("plan_project_export", { folderId });
  if ((plan.missing.length || plan.substitutions) && !await confirmMissing(missingMessage(plan))) return null;
  return invoke<{ status: string; path?: string; sizeBytes?: number }>("export_project", { folderId, parent });
}
