import { memo, useEffect, useRef, useState } from "react";
import { LocateFixed } from "lucide-react";
import { call } from "../../backend";
import type { LyricsDocument, Track } from "../../backend";
import type { Runtime } from "../../useBackend";
import { usePlayback } from "../../services/playbackState";
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
  const [lyrics, setLyrics] = useState<LyricsDocument | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState(false);
  const [retry, setRetry] = useState(0);
  const [follow, setFollow] = useState(true);
  const session_id = runtime.model.status!.context.session_id;
  const playback = usePlayback(runtime);
  useEffect(() => {
    let active = true;
    setLyrics(null);
    setError(false);
    setFollow(true);
    setLoading(!!track?.metadata.lyrics);
    if (track?.metadata.lyrics)
      void call("lyrics", { session_id, track_id: track.track_id })
        .then((value) => {
          if (active) setLyrics(value);
        })
        .catch(() => {
          if (active) setError(true);
        })
        .finally(() => {
          if (active) setLoading(false);
        });
    return () => {
      active = false;
    };
  }, [session_id, track?.track_id, track?.metadata.lyrics?.sha256, retry]);
  const position = playback.position_ms ?? 0;
  let active = -1;
  // LRC has no end time; SRT/VTT gaps keep all lines inactive.
  lyrics?.lines.forEach((line, index) => {
    if (
      line.start_ms !== null &&
      position >= line.start_ms &&
      (line.end_ms === null || position < line.end_ms)
    )
      active = index;
  });
  if (loading) return <Loading label="Загружаем текст…" />;
  if (error)
    return (
      <Empty
        title="Не удалось загрузить текст"
        action={
          <button onClick={() => setRetry((n) => n + 1)}>Повторить</button>
        }
      >
        Подключитесь к сети, чтобы загрузить текст, который ещё не сохранён на
        устройстве.
      </Empty>
    );
  if (!lyrics?.lines.length)
    return (
      <Empty
        title="Текста пока нет"
        action={
          <button disabled={!track} onClick={edit}>
            Добавить текст
          </button>
        }
      >
        Добавьте слова песни или файл с синхронизированным текстом.
      </Empty>
    );
  return (
    <LyricsLines
      lyrics={lyrics}
      active={active}
      follow={follow}
      setFollow={setFollow}
      seek={(value) =>
        void runtime.run(
          "Перейти к строке",
          () =>
            call("control_playback", {
              session_id,
              action: { kind: "seek", value },
            }),
          false,
        )
      }
    />
  );
}

const LyricsLines = memo(
  function LyricsLines({
    lyrics,
    active,
    follow,
    setFollow,
    seek,
  }: {
    lyrics: LyricsDocument;
    active: number;
    follow: boolean;
    setFollow: (value: boolean) => void;
    seek: (value: number) => void;
  }) {
    const scroller = useRef<HTMLDivElement>(null);
    const line = useRef<HTMLButtonElement>(null);
    const synced = lyrics.lines.some((entry) => entry.start_ms !== null);
    useEffect(() => {
      const container = scroller.current;
      const current = line.current;
      if (!follow || !container || !current || document.hidden) return;
      // Scroll only the lyrics viewport. scrollIntoView also moves parent dialogs.
      const top =
        current.getBoundingClientRect().top -
        container.getBoundingClientRect().top +
        container.scrollTop -
        (container.clientHeight - current.clientHeight) / 2;
      container.scrollTo({
        top,
        behavior:
          matchMedia("(prefers-reduced-motion: reduce)").matches ||
          document.documentElement.dataset.motion === "reduce"
            ? "instant"
            : "smooth",
      });
    }, [active, follow]);
    return (
      <div className="lyrics-panel">
        <div className="lyrics-caption">
          <span>
            {synced ? "Синхронизированный текст" : "Текст без синхронизации"}
          </span>
          {synced && !follow && (
            <button onClick={() => setFollow(true)}>
              <LocateFixed />К текущей строке
            </button>
          )}
        </div>
        <div
          ref={scroller}
          className={`lyrics-lines ${synced ? "synced" : "plain"}`}
          aria-label="Строки песни"
          tabIndex={0}
          onWheel={() => setFollow(false)}
          onTouchMove={() => setFollow(false)}
          onPointerDown={(e) => {
            if (e.pointerType === "mouse" && e.target === e.currentTarget)
              setFollow(false);
          }}
          onKeyDown={(e) => {
            if (
              [
                "ArrowDown",
                "ArrowUp",
                "PageDown",
                "PageUp",
                "Home",
                "End",
                " ",
              ].includes(e.key)
            )
              setFollow(false);
          }}
        >
          {lyrics.lines.map((entry, index) =>
            entry.start_ms !== null ? (
              <button
                key={index}
                ref={index === active ? line : undefined}
                className={index === active ? "current-line" : ""}
                aria-current={index === active ? "true" : undefined}
                onClick={() => {
                  seek(entry.start_ms!);
                  setFollow(true);
                }}
              >
                {entry.text || "\u00a0"}
              </button>
            ) : (
              <p key={index}>{entry.text || "\u00a0"}</p>
            ),
          )}
        </div>
      </div>
    );
  },
  (a, b) =>
    a.lyrics === b.lyrics && a.active === b.active && a.follow === b.follow,
);
