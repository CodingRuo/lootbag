import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { fileSrc } from "./fileSrc.ts";
import { repeatAudioLoop, resolveAudioLoop, seekAudio, type AudioLoop } from "./audioAudition";
import { errorMessage } from "./errorMessage.ts";
import { Icon } from "./ui.tsx";

type AudioWaveform = {
  durationSeconds: number;
  peaks: { startSeconds: number; endSeconds: number; min: number; max: number }[];
};

type EmbeddedAudioTags = {
  status: "ready" | "unsupported";
  title: string | null;
  artist: string | null;
  album: string | null;
  genre: string | null;
  message: string | null;
};

export type AudioMetadata = {
  status: "ready" | "error";
  durationSeconds: number | null;
  sampleRate: number | null;
  channels: number | null;
  codec: string | null;
  error: string | null;
};

type AudioFile = {
  id: number;
  originalFilename: string;
  audio: AudioMetadata | null;
};

const number = new Intl.NumberFormat("de-DE", { maximumFractionDigits: 2 });
// Loop fields show two decimals, rounded down so the end never exceeds the real duration.
const seconds = (value: number) => (Math.floor(value * 100) / 100).toFixed(2);

function time(value: number | null) {
  if (value === null || !Number.isFinite(value)) return "—";
  const seconds = Math.max(0, Math.floor(value));
  const minutes = Math.floor(seconds / 60);
  return `${minutes.toString().padStart(2, "0")}:${(seconds % 60).toString().padStart(2, "0")}`;
}


export default function AudioPreview({ file, onAnalysisComplete }: { file: AudioFile; onAnalysisComplete?: () => void }) {
  const analysisComplete = useRef(onAnalysisComplete);
  analysisComplete.current = onAnalysisComplete;
  const player = useRef<HTMLAudioElement>(null);
  const [url, setUrl] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [ready, setReady] = useState(false);
  const [playing, setPlaying] = useState(false);
  const [position, setPosition] = useState(0);
  const [duration, setDuration] = useState<number | null>(null);
  const [volume, setVolume] = useState(1);
  const [metadata, setMetadata] = useState(file.audio);
  const [analysisLoading, setAnalysisLoading] = useState(!file.audio);
  const [analysisError, setAnalysisError] = useState<string | null>(null);
  const [playbackError, setPlaybackError] = useState<string | null>(null);
  const [waveform, setWaveform] = useState<AudioWaveform | null>(null);
  const [waveformLoading, setWaveformLoading] = useState(true);
  const [waveformError, setWaveformError] = useState<string | null>(null);
  const [loop, setLoop] = useState<AudioLoop | null>(null);
  const loopRef = useRef<AudioLoop | null>(null);
  const [loopStart, setLoopStart] = useState("0");
  const [loopEnd, setLoopEnd] = useState("");
  const [loopError, setLoopError] = useState<string | null>(null);
  const waveformRequest = useRef<string | null>(null);
  const [embeddedTags, setEmbeddedTags] = useState<EmbeddedAudioTags | null>(null);
  const [tagsLoading, setTagsLoading] = useState(true);
  const [tagsError, setTagsError] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    const requestId = crypto.randomUUID();
    setEmbeddedTags(null);
    setTagsLoading(true);
    setTagsError(null);
    invoke<EmbeddedAudioTags>("audio_tags", { fileId: file.id, requestId })
      .then((tags) => { if (active) setEmbeddedTags(tags); })
      .catch((error: unknown) => { if (active) setTagsError(errorMessage(error)); })
      .finally(() => { if (active) setTagsLoading(false); });
    return () => {
      active = false;
      void invoke("cancel_audio_tags", { requestId }).catch(() => undefined);
    };
  }, [file.id]);

  useEffect(() => {
    let active = true;
    const element = player.current;
    setUrl(null);
    setLoading(true);
    setReady(false);
    setPlaying(false);
    setPosition(0);
    setDuration(null);
    setMetadata(file.audio);
    setAnalysisLoading(!file.audio);
    setAnalysisError(null);
    setPlaybackError(null);
    setWaveform(null);
    setWaveformLoading(true);
    setWaveformError(null);
    setLoop(null);
    loopRef.current = null;
    setLoopStart("0");
    setLoopEnd("");
    setLoopError(null);
    if (element) element.loop = false;
    const requestId = crypto.randomUUID();
    waveformRequest.current = requestId;
    invoke<string>("resolve_preview", { fileId: file.id })
      .then((path) => {
        if (active) setUrl(fileSrc(path));
      })
      .catch((error: unknown) => {
        if (active) setPlaybackError(errorMessage(error));
      })
      .finally(() => {
        if (active) setLoading(false);
      });
    // Saved metadata is reused; only never-analyzed files are analyzed on open.
    if (!file.audio) invoke<AudioMetadata>("analyze_audio", { fileId: file.id })
      .then((properties) => {
        if (active) {
          setMetadata(properties);
          analysisComplete.current?.();
        }
      })
      .catch((error: unknown) => {
        if (active) setAnalysisError(errorMessage(error));
      })
      .finally(() => {
        if (active) setAnalysisLoading(false);
      });
    invoke<AudioWaveform>("audio_waveform", { fileId: file.id, requestId })
      .then((result) => { if (active) setWaveform(result); })
      .catch((error: unknown) => { if (active) setWaveformError(errorMessage(error)); })
      .finally(() => { if (active) setWaveformLoading(false); });
    return () => {
      active = false;
      element?.pause();
      if (element) element.loop = false;
      void invoke("cancel_audio_waveform", { requestId }).catch(() => undefined);
    };
  }, [file.id]);

  useEffect(() => {
    if (!playing || !loop) return;
    let frame = 0;
    function tick() {
      enforceLoop();
      frame = requestAnimationFrame(tick);
    }
    frame = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(frame);
  }, [playing, loop]);

  function syncTime() {
    const element = player.current;
    if (!element) return;
    setPosition(Number.isFinite(element.currentTime) ? element.currentTime : 0);
    setDuration(
      Number.isFinite(element.duration) && element.duration > 0
        ? element.duration
        : null,
    );
  }

  async function togglePlayback() {
    const element = player.current;
    if (!element || !ready) return;
    if (element.paused) {
      try {
        const selection = loopRef.current;
        if (selection && (element.currentTime < selection.start || element.currentTime >= selection.end)) {
          seekAudio(element, selection.start);
        }
        await element.play();
      } catch (error: unknown) {
        setPlaybackError(`Wiedergabe fehlgeschlagen: ${errorMessage(error)}`);
      }
    } else element.pause();
  }

  function seek(position: number) {
    const element = player.current;
    if (!element || !ready) return;
    try { seekAudio(element, position); syncTime(); }
    catch (error: unknown) { setPlaybackError(`Position nicht erreichbar: ${errorMessage(error)}`); }
  }

  function enforceLoop() {
    const element = player.current;
    const selection = loopRef.current;
    if (!element || !selection || element.loop) return;
    try {
      if (repeatAudioLoop(element, selection)) {
        void element.play().catch((error: unknown) => setPlaybackError(`Wiedergabe fehlgeschlagen: ${errorMessage(error)}`));
      }
    } catch (error: unknown) { setLoopError(errorMessage(error)); }
  }

  function applyLoop(enabled: boolean, selection?: AudioLoop) {
    const element = player.current;
    if (!element || duration === null) return;
    try {
      const next = enabled ? resolveAudioLoop(duration, loopStart, loopEnd, selection) : null;
      loopRef.current = next;
      setLoop(next);
      element.loop = next?.start === 0 && next.end === duration;
      setLoopError(null);
      if (enabled && !element.paused) enforceLoop();
    } catch (error: unknown) { setLoopError(errorMessage(error)); }
  }

  const auditionReady = ready && duration !== null && playbackError === null;

  const metadataError = analysisError ?? metadata?.error;
  // Only populated tags are listed; the block disappears when a file has none.
  const tagFields = ([["title", "Titel"], ["artist", "Künstler"], ["album", "Album"], ["genre", "Genre"]] as const).filter(([field]) => embeddedTags?.[field]);
  return (
    <>
      <audio
        ref={player}
        hidden
        preload="metadata"
        src={url ?? undefined}
        aria-label={`Sound-Vorschau: ${file.originalFilename}`}
        onLoadStart={() => {
          setReady(false);
          setPlaying(false);
          setPosition(0);
          setDuration(null);
        }}
        onLoadedMetadata={() => {
          syncTime();
          const length = player.current?.duration;
          if (length && Number.isFinite(length)) setLoopEnd(seconds(length));
          // With metadata preload, play() must be allowed to request the remaining audio data.
          setReady(true);
        }}
        onDurationChange={syncTime}
        onTimeUpdate={() => { enforceLoop(); syncTime(); }}
        onCanPlay={() => setReady(true)}
        onPlay={() => setPlaying(true)}
        onPause={() => setPlaying(false)}
        onEnded={() => {
          if (loopRef.current) enforceLoop();
          else setPlaying(false);
          syncTime();
        }}
        onVolumeChange={() => setVolume(player.current?.volume ?? 1)}
        onError={() => {
          setReady(false);
          setPlaying(false);
          setPlaybackError(
            "Dieser Sound kann nicht abgespielt werden. Die Datei ist möglicherweise beschädigt oder ihr Audiocodec wird nicht unterstützt.",
          );
        }}
      />
      <div className="audio-controls">
        <div className="audio-transport">
          <button
            className="button outlined"
            onClick={() => void togglePlayback()}
            disabled={!ready || playbackError !== null}
            aria-label={playing ? "Sound pausieren" : "Sound abspielen"}
          >
            <Icon name={playing ? "pause" : "play"} />
            {playing ? "Pause" : "Abspielen"}
          </button>
          <output aria-label="Wiedergabezeit">
            {time(position)} / {time(duration)}
          </output>
        </div>
        {waveform && <div
          className="audio-waveform"
          role="slider"
          tabIndex={auditionReady ? 0 : -1}
          aria-label="Wellenform: Abspielposition"
          aria-valuemin={0}
          aria-valuemax={duration ?? waveform.durationSeconds}
          aria-valuenow={position}
          aria-valuetext={`${time(position)} von ${time(duration)}`}
          aria-disabled={!auditionReady}
          onPointerDown={(event) => {
            if (!auditionReady || duration === null) return;
            const bounds = event.currentTarget.getBoundingClientRect();
            seek(Math.max(0, Math.min(1, (event.clientX - bounds.left) / bounds.width)) * duration);
            event.currentTarget.focus();
          }}
          onKeyDown={(event) => {
            if (!auditionReady || duration === null) return;
            const step = event.shiftKey ? 5 : 0.1;
            const next = event.key === "ArrowLeft" ? position - step : event.key === "ArrowRight" ? position + step : event.key === "Home" ? 0 : event.key === "End" ? duration : null;
            if (next !== null) { event.preventDefault(); seek(next); }
          }}
        >
          <svg viewBox="0 0 1000 100" preserveAspectRatio="none" aria-hidden="true">
            {loop && <rect className="audio-waveform-selection" x={loop.start / (duration ?? 1) * 1000} y="0" width={(loop.end - loop.start) / (duration ?? 1) * 1000} height="100" />}
            <line className="audio-waveform-center" x1="0" x2="1000" y1="50" y2="50" />
            {waveform.peaks.map((peak, index) => <rect key={index}
              x={peak.startSeconds / waveform.durationSeconds * 1000}
              y={50 - peak.max * 46}
              width={Math.max(0.8, (peak.endSeconds - peak.startSeconds) / waveform.durationSeconds * 1000)}
              height={Math.max(1, (peak.max - peak.min) * 46)}
            />)}
            <line className="audio-waveform-cursor" x1={position / (duration ?? waveform.durationSeconds) * 1000} x2={position / (duration ?? waveform.durationSeconds) * 1000} y1="0" y2="100" />
          </svg>
        </div>}
        {waveformLoading && <div className="audio-waveform-status" role="status">
          <span>Wellenform wird analysiert …</span>
          <button className="button subtle compact" onClick={() => {
            if (waveformRequest.current) void invoke("cancel_audio_waveform", { requestId: waveformRequest.current }).catch((error: unknown) => setWaveformError(errorMessage(error)));
          }}>Abbrechen</button>
        </div>}
        {waveformError && <p className="inline-error" role="status">Wellenform: {waveformError}</p>}
        <dl className="audio-facts" aria-label="Audioeigenschaften">
          <div><dt>Dauer</dt><dd>{metadata?.durationSeconds != null ? `${number.format(metadata.durationSeconds)} s` : "—"}</dd></div>
          <div><dt>Samplerate</dt><dd>{metadata?.sampleRate != null ? `${number.format(metadata.sampleRate)} Hz` : "—"}</dd></div>
          <div><dt>Kanäle</dt><dd>{metadata?.channels != null ? metadata.channels === 1 ? "Mono" : metadata.channels === 2 ? "Stereo" : number.format(metadata.channels) : "—"}</dd></div>
          <div><dt>Codec</dt><dd>{metadata?.codec ?? "—"}</dd></div>
        </dl>
        {analysisLoading && <p className="detail-hint" role="status">Eigenschaften werden gelesen …</p>}
        {metadataError && <p className="inline-error" role="alert">Analysefehler: {metadataError}</p>}
        <label>
          <span>Abspielposition</span>
          <input
            type="range"
            min="0"
            max={duration ?? 0}
            step="0.1"
            value={Math.min(position, duration ?? 0)}
            disabled={!ready || duration === null || playbackError !== null}
            aria-valuetext={`${time(position)} von ${time(duration)}`}
            onChange={(event) => {
              seek(Number(event.target.value));
            }}
          />
        </label>
        <label>
          <span>Lautstärke <output>{Math.round(volume * 100)} %</output></span>
          <input
            type="range"
            min="0"
            max="1"
            step="0.05"
            value={volume}
            aria-valuetext={`${Math.round(volume * 100)} Prozent`}
            onChange={(event) => {
              if (player.current) player.current.volume = Number(event.target.value);
            }}
          />
        </label>
        <details className="audio-loop">
          <summary>Loop-Vorschau</summary>
          <fieldset disabled={!auditionReady}>
          <div className="audio-loop-times">
            <label><span>Anfang (Sekunden)</span><input type="number" min="0" max={duration ?? 0} step="0.01" value={loopStart} onChange={(event) => setLoopStart(event.target.value)} /></label>
            <label><span>Ende (Sekunden)</span><input type="number" min="0" max={duration ?? 0} step="0.01" value={loopEnd} onChange={(event) => setLoopEnd(event.target.value)} /></label>
          </div>
          <div className="audio-loop-actions">
            <button className="button outlined compact" onClick={() => applyLoop(true)}>Bereich wiederholen</button>
            <button className="button subtle compact" onClick={() => {
              if (duration === null) return;
              setLoopStart("0"); setLoopEnd(seconds(duration));
              applyLoop(true, { start: 0, end: duration });
            }}>Ganzer Sound</button>
            {loop && <button className="button subtle compact" onClick={() => applyLoop(false)}>Loop aus</button>}
          </div>
          {loop && <p className="detail-hint" role="status">Loop aktiv: {number.format(loop.start)}–{number.format(loop.end)} s</p>}
          {loopError && <p className="inline-error" role="alert">{loopError}</p>}
          </fieldset>
        </details>
      </div>
      {(loading || (!ready && !playbackError)) && <p className="detail-hint" role="status">{loading ? "Vorschau wird geladen …" : "Audiodaten werden geladen …"}</p>}
      {playbackError && <p className="inline-error" role="alert">{playbackError}</p>}
      {(tagsLoading || tagsError || embeddedTags?.message || tagFields.length > 0) && <div className="audio-properties">
        <h3>Eingebettete Audio-Tags</h3>
        {tagFields.length > 0 && <dl>
          {tagFields.map(([field, label]) => <div key={field}>
            <dt>{label}</dt>
            <dd>{embeddedTags![field]}</dd>
          </div>)}
        </dl>}
        {tagsLoading && <p className="detail-hint" role="status">Eingebettete Tags werden gelesen …</p>}
        {embeddedTags?.message && <p className="detail-hint" role="status">{embeddedTags.message}</p>}
        {tagsError && <p className="inline-error" role="alert">Eingebettete Tags nicht verfügbar: {tagsError}</p>}
      </div>}
    </>
  );
}
