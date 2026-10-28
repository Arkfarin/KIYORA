import { useEffect, useRef, useState } from "react";
import {
  ChevronDown,
  Heart,
  ListMusic,
  Maximize2,
  Mic2,
  MoreHorizontal,
  Pause,
  Play,
  Repeat,
  Repeat1,
  Shuffle,
  SkipBack,
  SkipForward,
  Volume2,
  VolumeX,
  X,
} from "lucide-react";
import { call, duration, mutate } from "../../backend";
import type { PlaybackAction, QueueAction, Track } from "../../backend";
import type { Runtime } from "../../useBackend";
import { resumePlayback } from "../../services/playback";
import { acquireArtwork } from "../../services/artwork";
import type { CSSProperties } from "react";
import { Cover } from "../../components/music/Cover";
import { IconButton, Modal } from "../../components/ui/Primitives";
import { QueuePanel } from "./QueuePanel";
import { LyricsView } from "../lyrics/LyricsView";
export function Player({
  runtime,
  onMenu,
  onEdit,
}: {
  runtime: Runtime;
  onMenu: (track: Track) => void;
  onEdit: (track: Track) => void;
}) {
  const { model, run } = runtime;
  const session_id = model.status!.context.session_id;
  const [full, setFull] = useState(false);
  const [tone, setTone] = useState<string>();
  const [panel, setPanel] = useState<"queue" | "lyrics" | null>(null);
  const [seek, setSeek] = useState<number | null>(null);
  const [volume, setVolume] = useState(model.info?.playback_volume ?? 1);
  const [previousVolume, setPreviousVolume] = useState(1);
  const startY = useRef<number | null>(null);
  const playback = model.playback;
  const queue = model.queue;
  const track =
    model.projection?.state.tracks[
      playback.track_id ?? queue?.current_track_id ?? ""
    ];
  const liked =
    !!track &&
    !!model.projection?.state.favorites.liked?.includes(track.track_id);
  useEffect(() => {
    let active = true;
    setTone(undefined);
    const sha = track?.metadata.artwork?.sha256;
    if (full && track && sha) {
      void acquireArtwork(session_id, track.track_id, sha)
        .then(async (asset) => {
          if (!asset) return;
          try {
            const color = await asset.tone();
            if (active) setTone(color);
          } finally {
            asset.release();
          }
        })
        .catch(() => {});
    }
    return () => {
      active = false;
    };
  }, [full, session_id, track?.track_id, track?.metadata.artwork?.sha256]);
  const active = ["playing", "paused", "buffering"].includes(playback.state);
  const control = (action: PlaybackAction) =>
    void run(
      "Управление плеером",
      () => call("control_playback", { session_id, action }),
      false,
    );
  const editQueue = (action: QueueAction) => {
    if (queue)
      void run(
        "Настройка очереди",
        () =>
          call("edit_queue", {
            session_id,
            queue_id: queue.queue_id,
            revision: queue.revision,
            action,
          }),
        false,
      );
  };
  const togglePlay = () => {
    if (active)
      control({ kind: playback.state === "playing" ? "pause" : "resume" });
    else if (track)
      void run(
        "Воспроизведение",
        () => resumePlayback(session_id, playback, queue),
        false,
      );
  };
  const durationMs = playback.duration_ms ?? track?.metadata.duration_ms ?? 0;
  useEffect(() => {
    if (model.info) setVolume(model.info.playback_volume);
  }, [model.info?.playback_volume]);
  useEffect(() => {
    setSeek(null);
  }, [playback.track_id]);
  const repeat = queue?.repeat ?? "off";
  const buttons = (
    <div className="transport-buttons">
      <IconButton
        label={
          queue?.shuffle ? "Выключить случайный порядок" : "Случайный порядок"
        }
        aria-pressed={queue?.shuffle ?? false}
        disabled={!queue?.total}
        onClick={() => editQueue({ kind: "shuffle", enabled: !queue?.shuffle })}
      >
        <Shuffle />
      </IconButton>
      <IconButton
        label="Предыдущая композиция"
        disabled={!queue?.total || (!queue?.position && repeat === "off")}
        onClick={() => control({ kind: "previous" })}
      >
        <SkipBack fill="currentColor" />
      </IconButton>
      <IconButton
        className="play-button"
        label={playback.state === "playing" ? "Пауза" : "Слушать"}
        disabled={!track}
        onClick={togglePlay}
      >
        {playback.state === "playing" ? (
          <Pause fill="currentColor" />
        ) : (
          <Play fill="currentColor" />
        )}
      </IconButton>
      <IconButton
        label="Следующая композиция"
        disabled={
          !queue?.total ||
          (queue.position === queue.total - 1 && repeat === "off")
        }
        onClick={() => control({ kind: "next" })}
      >
        <SkipForward fill="currentColor" />
      </IconButton>
      <IconButton
        label={`Повтор: ${repeat === "off" ? "выключен" : repeat === "track" ? "композиции" : "очереди"}`}
        aria-pressed={repeat !== "off"}
        disabled={!queue?.total}
        onClick={() =>
          editQueue({
            kind: "repeat",
            mode:
              repeat === "off" ? "queue" : repeat === "queue" ? "track" : "off",
          })
        }
      >
        {repeat === "track" ? <Repeat1 /> : <Repeat />}
      </IconButton>
    </div>
  );
  const progress = (
    <div className="seek-control">
      <span>{duration(seek ?? playback.position_ms)}</span>
      <input
        type="range"
        aria-label="Позиция воспроизведения"
        min={0}
        max={Math.max(1, durationMs)}
        step={1000}
        value={Math.min(seek ?? playback.position_ms ?? 0, durationMs)}
        disabled={!active || !durationMs}
        onChange={(e) => setSeek(Number(e.target.value))}
        onPointerUp={(e) => {
          control({ kind: "seek", value: Number(e.currentTarget.value) });
          setSeek(null);
        }}
        onKeyUp={(e) => {
          if (
            [
              "ArrowLeft",
              "ArrowRight",
              "ArrowUp",
              "ArrowDown",
              "Home",
              "End",
              "PageUp",
              "PageDown",
            ].includes(e.key)
          ) {
            control({ kind: "seek", value: Number(e.currentTarget.value) });
            setSeek(null);
          }
        }}
        onBlur={() => setSeek(null)}
      />
      <span>{duration(durationMs)}</span>
    </div>
  );
  const like = (
    <IconButton
      label={liked ? "Убрать из любимых" : "В любимые"}
      aria-pressed={liked}
      disabled={!track || model.status?.mode !== "ONLINE_READY"}
      onClick={() =>
        track &&
        void run("Избранное", () =>
          mutate(session_id, track.track_id, {
            action: "favorite.set",
            payload: { set_name: "liked", value: !liked },
          }),
        )
      }
    >
      <Heart fill={liked ? "currentColor" : "none"} />
    </IconButton>
  );
  const panelContent =
    panel === "queue" ? (
      <QueuePanel runtime={runtime} />
    ) : panel === "lyrics" ? (
      <LyricsView
        runtime={runtime}
        track={track}
        edit={() => {
          if (track) onEdit(track);
        }}
      />
    ) : null;
  return (
    <>
      <footer
        className="player"
        aria-label="Аудиоплеер"
        onPointerDown={(e) => {
          if (e.pointerType === "touch") startY.current = e.clientY;
        }}
        onPointerUp={(e) => {
          if (startY.current !== null && startY.current - e.clientY > 55)
            setFull(true);
          startY.current = null;
        }}
      >
        <div className="player-track">
          <button
            className="now-cover"
            aria-label="Открыть текущую композицию"
            onClick={() => setFull(true)}
          >
            <Cover track={track} session={session_id} />
          </button>
          <button className="now-label" onClick={() => setFull(true)}>
            <strong>{track?.metadata.title || "Выберите свою музыку"}</strong>
            <span>
              {playback.state === "failed"
                ? "Не удалось воспроизвести — нажмите Play для повтора"
                : playback.state === "buffering"
                  ? "Подготовка аудио…"
                  : track?.metadata.performer || "Ваша личная коллекция"}
            </span>
          </button>
          <span className="desktop-like">{like}</span>
        </div>
        <div className="desktop-transport">
          {buttons}
          {progress}
        </div>
        <div className="player-utilities">
          <IconButton
            label="Текст песни"
            aria-pressed={panel === "lyrics"}
            onClick={() => setPanel((p) => (p === "lyrics" ? null : "lyrics"))}
          >
            <Mic2 />
          </IconButton>
          <IconButton
            label="Очередь воспроизведения"
            aria-pressed={panel === "queue"}
            onClick={() => setPanel((p) => (p === "queue" ? null : "queue"))}
          >
            <ListMusic />
          </IconButton>
          <IconButton
            label={volume ? "Выключить звук" : "Включить звук"}
            onClick={() => {
              const v = volume ? 0 : previousVolume;
              if (volume) setPreviousVolume(volume);
              setVolume(v);
              control({ kind: "set_volume", value: v });
            }}
          >
            {volume ? <Volume2 /> : <VolumeX />}
          </IconButton>
          <input
            aria-label="Громкость"
            className="volume-slider"
            type="range"
            min={0}
            max={1}
            step={0.01}
            value={volume}
            onChange={(e) => setVolume(Number(e.target.value))}
            onPointerUp={(e) =>
              control({
                kind: "set_volume",
                value: Number(e.currentTarget.value),
              })
            }
            onKeyUp={(e) =>
              control({
                kind: "set_volume",
                value: Number(e.currentTarget.value),
              })
            }
          />
          <IconButton label="На весь экран" onClick={() => setFull(true)}>
            <Maximize2 />
          </IconButton>
        </div>
        <div className="mobile-transport">
          <IconButton
            label={playback.state === "playing" ? "Пауза" : "Слушать"}
            disabled={!track}
            onClick={togglePlay}
          >
            {playback.state === "playing" ? (
              <Pause fill="currentColor" />
            ) : (
              <Play fill="currentColor" />
            )}
          </IconButton>
        </div>
        <progress
          className="mini-progress"
          aria-label="Прогресс воспроизведения"
          max={Math.max(1, durationMs)}
          value={playback.position_ms ?? 0}
        />
      </footer>
      {panel && !full && (
        <Modal
          title={panel === "queue" ? "Очередь воспроизведения" : "Текст песни"}
          onClose={() => setPanel(null)}
        >
          {panelContent}
        </Modal>
      )}
      {full && (
        <Modal title="Сейчас играет" fullScreen onClose={() => setFull(false)}>
          <div
            className="now-playing-full"
            style={
              { "--artwork-tone": tone ?? "var(--surface)" } as CSSProperties
            }
          >
            <div className="now-artwork">
              <Cover track={track} session={session_id} />
            </div>
            <div className="now-controls">
              <div className="now-title">
                <div>
                  <h2>{track?.metadata.title ?? "Выберите композицию"}</h2>
                  <p>
                    {track?.metadata.performer || "Ваша музыка всегда рядом"}
                  </p>
                </div>
                {like}
              </div>
              {progress}
              {buttons}
              <div className="now-tools">
                <button
                  aria-pressed={panel === "lyrics"}
                  onClick={() =>
                    setPanel((p) => (p === "lyrics" ? null : "lyrics"))
                  }
                >
                  <Mic2 />
                  Текст
                </button>
                <button
                  aria-pressed={panel === "queue"}
                  onClick={() =>
                    setPanel((p) => (p === "queue" ? null : "queue"))
                  }
                >
                  <ListMusic />
                  Очередь
                </button>
                <IconButton
                  label="Действия с композицией"
                  disabled={!track}
                  onClick={() => track && onMenu(track)}
                >
                  <MoreHorizontal />
                </IconButton>
                <IconButton
                  label="Свернуть плеер"
                  onClick={() => setFull(false)}
                >
                  <ChevronDown />
                </IconButton>
              </div>
              {playback.state === "failed" && (
                <p role="alert" className="inline-error">
                  {playback.code}. Повторите воспроизведение или проверьте
                  подключение в настройках.
                </p>
              )}
            </div>
          </div>
          {panel && (
            <div className="now-panel">
              <div className="section-heading">
                <span />
                <IconButton
                  label="Закрыть панель"
                  onClick={() => setPanel(null)}
                >
                  <X />
                </IconButton>
              </div>
              {panelContent}
            </div>
          )}
        </Modal>
      )}
    </>
  );
}
