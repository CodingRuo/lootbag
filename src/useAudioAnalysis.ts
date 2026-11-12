import { useEffect, useRef, useState, type RefObject } from "react";
import { Channel, invoke } from "@tauri-apps/api/core";
import { errorMessage } from "./errorMessage.ts";
import type { CatalogQuery } from "./useCatalogQuery.ts";
import { PAGE_SIZE, type Act, type AudioAnalysis, type AudioAnalysisProgress, type LibraryInfo, type Notice, type Setter } from "./ui.tsx";

// Scope-bound "analyse all matching sounds" job: prepare, run with progress, stop and discard.
export function useAudioAnalysis({ library, query, revision, actionsDisabled, operationRunning, act, setNotice }: {
  library: LibraryInfo | null;
  query: CatalogQuery;
  revision: number;
  actionsDisabled: boolean;
  operationRunning: RefObject<boolean>;
  act: Act;
  setNotice: Setter<Notice | null>;
}) {
  const { text, packageId, folderId, originalFolder, format, kind, workflow, tag, audioFilters, audioCriteria, audioFilterError, setAudioResultsStale } = query;
  const [audioAnalysis, setAudioAnalysis] = useState<AudioAnalysis | null>(null);
  const audioAnalysisOwner = useRef<{ requestId: string; scope: string } | null>(null);
  const audioAnalysisScope = JSON.stringify([library?.root, text, packageId, folderId, originalFolder, format, kind, workflow, tag, audioFilters, revision]);
  const currentAudioAnalysisScope = useRef(audioAnalysisScope);
  currentAudioAnalysisScope.current = audioAnalysisScope;

  useEffect(() => {
    setAudioAnalysis(null);
    return () => {
      const owner = audioAnalysisOwner.current;
      if (owner?.scope === audioAnalysisScope) {
        audioAnalysisOwner.current = null;
        void invoke("cancel_audio_analysis", { requestId: owner.requestId }).catch(() => {});
      }
    };
  }, [audioAnalysisScope]);

  function ownsAudioAnalysis(owner: { requestId: string; scope: string }) {
    return audioAnalysisOwner.current === owner && currentAudioAnalysisScope.current === owner.scope;
  }

  async function prepareAudioAnalysis() {
    if (!library || kind !== "sound" || actionsDisabled || audioFilterError || operationRunning.current) return;
    const previous = audioAnalysisOwner.current;
    if (previous) void invoke("cancel_audio_analysis", { requestId: previous.requestId }).catch(() => {});
    const owner = { requestId: crypto.randomUUID(), scope: audioAnalysisScope };
    audioAnalysisOwner.current = owner;
    setAudioAnalysis({ requestId: owner.requestId, phase: "preparing", total: 0, progress: null, stopping: false, message: null });
    try {
      const prepared = await invoke<{ requestId: string; total: number }>("prepare_audio_analysis", {
        requestId: owner.requestId,
        audioCriteria,
        query: { text, packageId, folderId, originalFolder, format, kind, workflow, tag, offset: 0, limit: PAGE_SIZE },
      });
      if (!ownsAudioAnalysis(owner)) return;
      if (prepared.requestId !== owner.requestId) throw new Error("Die Analysevorbereitung gehört nicht zum aktuellen Auftrag.");
      setAudioAnalysis({ requestId: owner.requestId, phase: "prepared", total: prepared.total, progress: null, stopping: false, message: null });
    } catch (error: unknown) {
      if (ownsAudioAnalysis(owner)) setAudioAnalysis({ requestId: owner.requestId, phase: "failed", total: 0, progress: null, stopping: false, message: errorMessage(error) });
    }
  }

  function startAudioAnalysis() {
    const owner = audioAnalysisOwner.current;
    const prepared = audioAnalysis;
    if (!owner || !ownsAudioAnalysis(owner) || prepared?.phase !== "prepared" || prepared.requestId !== owner.requestId || prepared.total === 0) return;
    void act("Sounds im Suchbereich werden analysiert …", async () => {
      const initial: AudioAnalysisProgress = { requestId: owner.requestId, total: prepared.total, processed: 0, ready: 0, errors: 0, skipped: 0, cancelled: false };
      setAudioAnalysis({ ...prepared, phase: "running", progress: initial });
      const progress = new Channel<AudioAnalysisProgress>();
      let receiving = true;
      progress.onmessage = (update) => {
        if (receiving && ownsAudioAnalysis(owner) && update.requestId === owner.requestId) {
          setAudioAnalysis((previous) => previous?.requestId === owner.requestId ? { ...previous, progress: update } : previous);
        }
      };
      try {
        const result = await invoke<AudioAnalysisProgress>("run_audio_analysis", { requestId: owner.requestId, progress });
        if (!ownsAudioAnalysis(owner)) return;
        if (result.requestId !== owner.requestId) throw new Error("Das Analyseergebnis gehört nicht zum aktuellen Auftrag.");
        setAudioAnalysis({ ...prepared, phase: "completed", progress: result, stopping: false });
        if (result.ready + result.errors > 0) setAudioResultsStale(true);
      } catch (error: unknown) {
        if (ownsAudioAnalysis(owner)) {
          setAudioAnalysis((previous) => previous ? { ...previous, phase: "failed", stopping: false, message: errorMessage(error) } : previous);
          setAudioResultsStale(true);
        }
      } finally {
        receiving = false;
      }
    });
  }

  async function stopAudioAnalysis() {
    const owner = audioAnalysisOwner.current;
    if (!owner || !ownsAudioAnalysis(owner) || audioAnalysis?.phase !== "running" || audioAnalysis.stopping) return;
    setAudioAnalysis((previous) => previous ? { ...previous, stopping: true, message: null } : previous);
    try {
      await invoke("cancel_audio_analysis", { requestId: owner.requestId });
    } catch (error: unknown) {
      if (ownsAudioAnalysis(owner)) setAudioAnalysis((previous) => previous?.phase === "running" ? { ...previous, stopping: false, message: `Abbruch nicht möglich: ${errorMessage(error)}` } : previous);
    }
  }

  async function discardAudioAnalysis() {
    const owner = audioAnalysisOwner.current;
    if (!owner) return;
    audioAnalysisOwner.current = null;
    setAudioAnalysis(null);
    try {
      await invoke("cancel_audio_analysis", { requestId: owner.requestId });
    } catch (error: unknown) {
      if (currentAudioAnalysisScope.current === owner.scope && audioAnalysisOwner.current === null) setNotice({ tone: "error", message: errorMessage(error) });
    }
  }

  return { audioAnalysis, prepareAudioAnalysis, startAudioAnalysis, stopAudioAnalysis, discardAudioAnalysis };
}
