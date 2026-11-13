import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { fileSrc } from "./fileSrc.ts";
import { loadFontFace } from "./fontFace.ts";
import { errorMessage } from "./errorMessage.ts";
import { Icon } from "./ui.tsx";

type FontFile = { id: number; sizeBytes: number; available: boolean };
const SIZES = [12, 18, 24, 36, 48, 72];

function useFont(file: FontFile, enabled: boolean) {
  const [state, setState] = useState<{ family?: string; error?: string }>({});
  useEffect(() => {
    setState({});
    if (!enabled) return;
    if (!file.available) { setState({ error: "Schriftdatei nicht verfügbar." }); return; }
    let active = true;
    let release = () => {};
    invoke<string>("resolve_preview", { fileId: file.id })
      .then((path) => loadFontFace(fileSrc(path), file.sizeBytes))
      .then((font) => { if (!active) { font.release(); return; } release = font.release; setState({ family: font.family }); })
      .catch((error) => { if (active) setState({ error: errorMessage(error, "Schrift nicht lesbar.") }); });
    return () => { active = false; release(); };
  }, [file.id, file.sizeBytes, file.available, enabled]);
  return state;
}

// Cards load their font only while visible and unload it when scrolled away or unmounted.
export function FontThumbnail({ file }: { file: FontFile }) {
  const host = useRef<HTMLSpanElement>(null);
  const [visible, setVisible] = useState(false);
  useEffect(() => {
    if (!host.current) return;
    const observer = new IntersectionObserver(([entry]) => setVisible(entry.isIntersecting), { root: host.current.closest(".catalog-scroll") });
    observer.observe(host.current);
    return () => observer.disconnect();
  }, []);
  const { family, error } = useFont(file, visible);
  return <span ref={host} className="font-thumbnail" title={error}>
    {family ? <span className="font-sample" style={{ fontFamily: `"${family}"` }}>Aa Bb 123</span> : <Icon name="font" />}
    {error && <span className="model-thumbnail-status">Keine Vorschau</span>}
  </span>;
}

export default function FontPreview({ file }: { file: FontFile }) {
  const [text, setText] = useState("Aa Bb 123 · Der schnelle Fuchs");
  const { family, error } = useFont(file, true);
  return <div className="font-preview">
    <input type="text" aria-label="Beispieltext" value={text} onChange={(event) => setText(event.target.value)} />
    {error ? <p className="inline-error" role="alert">{error}</p>
      : !family ? <p className="detail-hint" role="status">Schrift wird geladen …</p>
      : SIZES.map((size) => <p key={size} className="font-sample" style={{ fontFamily: `"${family}"`, fontSize: size }} title={`${size} px`}>{text || "Aa Bb 123"}</p>)}
  </div>;
}
