import { useEffect, useRef, useState } from "react";
import { call, errorOf } from "../../backend";
import type { LyricsDocument, Track } from "../../backend";
import type { Runtime } from "../../useBackend";
import { Empty, Loading } from "../../components/ui/Primitives";
export function LyricsView({
  runtime,
  track,
  edit,
}: {
  runtime: Runtime;
  track?: Track;
  edit: () => void;
}) {
  const [document, setDocument] = useState<LyricsDocument | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");
  const [follow, setFollow] = useState(true);
  const activeRef = useRef<HTMLButtonElement>(null);
  const session_id = runtime.model.status!.context.session_id;
  useEffect(() => {
    let active = true;
    setDocument(null);
    setError("");
    setLoading(!!track?.metadata.lyrics);
    if (track?.metadata.lyrics)
      void call("lyrics", { session_id, track_id: track.track_id })
        .then((value) => {
          if (active) setDocument(value);
        })
        .catch((e) => {
          if (active) setError(errorOf(e).message);
        })
        .finally(() => {
          if (active) setLoading(false);
        });
    return () => {
      active = false;
    };
  }, [session_id, track?.track_id, track?.metadata.lyrics?.sha256]);
  const position = runtime.model.playback.position_ms ?? 0;
  let active = -1;
  document?.lines.forEach((line, index) => {
    if (
      line.start_ms !== null &&
      position >= line.start_ms &&
      (line.end_ms === null || position <= line.end_ms)
    )
      active = index;
  });
  useEffect(() => {
    if (follow)
      activeRef.current?.scrollIntoView({
        block: "center",
        behavior:
          matchMedia("(prefers-reduced-motion: reduce)").matches ||
          documentElementReduced()
            ? "instant"
            : "smooth",
      });
  }, [active, follow]);
  if (loading) return <Loading label="Загружаем текст…" />;
  if (error)
    return (
      <div className="inline-error" role="alert">
        <p>{error}</p>
        <p>Текст доступен после загрузки его отдельного документа из Vault.</p>
      </div>
    );
  if (!document?.lines.length)
    return (
      <Empty
        title="Здесь может быть текст песни"
        action={
          <button disabled={!track} onClick={edit}>
            Добавить текст
          </button>
        }
      >
        Вставьте текст или импортируйте LRC, SRT, WebVTT.
      </Empty>
    );
  const synced = document.lines.some((line) => line.start_ms !== null);
  return (
    <div className="lyrics-panel">
      <div className="section-heading">
        <h3>Текст песни</h3>
        {synced && (
          <label className="check">
            <input
              type="checkbox"
              checked={follow}
              onChange={(e) => setFollow(e.target.checked)}
            />
            Следить за текстом
          </label>
        )}
      </div>
      <div
        className={`lyrics-lines ${synced ? "synced" : ""}`}
        aria-label="Текст песни"
      >
        {document.lines.map((line, index) =>
          line.start_ms !== null ? (
            <button
              key={index}
              ref={index === active ? activeRef : undefined}
              className={index === active ? "current-line" : ""}
              aria-current={index === active ? "true" : undefined}
              onClick={() =>
                void runtime.run(
                  "Перейти к строке",
                  () =>
                    call("control_playback", {
                      session_id,
                      action: { kind: "seek", value: line.start_ms! },
                    }),
                  false,
                )
              }
            >
              {line.text || "\u00a0"}
            </button>
          ) : (
            <p key={index}>{line.text || "\u00a0"}</p>
          ),
        )}
      </div>
    </div>
  );
}
function documentElementReduced() {
  return document.documentElement.dataset.motion === "reduce";
}
