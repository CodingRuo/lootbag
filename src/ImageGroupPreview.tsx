import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import MaterialPreview from "./MaterialPreview";
import SpriteSequencePreview from "./SpriteSequencePreview";
import type { ImageGroup, ImageWorkflow } from "./imageWorkflows";
import type { ModelExportSelection } from "./ModelPreview";

export default function ImageGroupPreview({ fileId, workflow, visible, onReady }: {
  fileId: number;
  workflow: ImageWorkflow;
  visible: boolean;
  onReady: (selection: ModelExportSelection | null) => void;
}) {
  const [group, setGroup] = useState<ImageGroup | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let active = true;
    setGroup(null);
    setError(null);
    onReady(null);
    void invoke<ImageGroup>("image_group", { fileId, workflow }).then((result) => {
      if (active) setGroup(result);
    }).catch((reason: unknown) => {
      if (active) setError(typeof reason === "string" ? reason : "Die Bildgruppe ist nicht verfügbar.");
    });
    return () => { active = false; onReady(null); };
  }, [fileId, workflow, onReady]);

  if (error) return <p className="inline-error" role="alert">{error}</p>;
  if (!group) return <p className="detail-hint" role="status">{workflow === "material" ? "Material-Set" : "Sprite-Sequenz"} wird geladen …</p>;
  return workflow === "material" ? <MaterialPreview group={group} visible={visible} onReady={onReady} /> : <SpriteSequencePreview group={group} visible={visible} onReady={onReady} />;
}
