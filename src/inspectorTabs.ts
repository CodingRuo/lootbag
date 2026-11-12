const tabs = ["preview", "properties", "origin"] as const;
export type InspectorTab = typeof tabs[number];

export function nextInspectorTab(current: InspectorTab, key: string): InspectorTab | null {
  if (key === "Home") return "preview";
  if (key === "End") return "origin";
  if (key === "ArrowRight") return tabs[(tabs.indexOf(current) + 1) % tabs.length];
  if (key === "ArrowLeft") return tabs[(tabs.indexOf(current) + tabs.length - 1) % tabs.length];
  return null;
}
