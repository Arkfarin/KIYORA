import { useEffect, useRef, useState } from "react";
import {
  Disc3,
  Heart,
  ListMusic,
  Maximize2,
  Mic2,
  MoreHorizontal,
  Pause,
  Play,
  SkipForward,
} from "lucide-react";
import { call, mutate } from "../../backend";
import type { PlaybackAction, QueueAction, Track } from "../../backend";
import type { Runtime } from "../../useBackend";
import { resumePlayback } from "../../services/playback";
import { usePlayback } from "../../services/playbackState";
import { Cover } from "../../components/music/Cover";
import { IconButton, Modal } from "../../components/ui/Primitives";
import { QueuePanel } from "./QueuePanel";
import { LyricsView } from "../lyrics/LyricsView";
import { SeekControl } from "./SeekControl";
import { Transport } from "./Transport";

type View = "artwork" | "lyrics" | "queue";
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
  const [view, setView] = useState<View>("artwork");
  const [queueBusy, setQueueBusy] = useState(false);
  const [queueError, setQueueError] = useState(false);
  const queueLock = useRef(false);
  const gesture = useRef<{ x: number; y: number } | null>(null);
  const playback = usePlayback(runtime);
  const queue = model.queue;
  const track =
    model.projection?.state.tracks[
      playback.track_id ?? queue?.current_track_id ?? ""
    ];
  const liked =
    !!track &&
    !!model.projection?.state.favorites.liked?.includes(track.track_id);
  const playing =
    playback.state === "playing" || playback.state === "buffering";
  const durationMs = playback.duration_ms ?? track?.metadata.duration_ms ?? 0;
  useEffect(() => {
    setFull(false);
    setView("artwork");
  }, [session_id]);
  useEffect(() => {
    const close = () => setFull(false);
    window.addEventListener("kiyora:navigate", close);
    return () => window.removeEventListener("kiyora:navigate", close);
  }, []);
  const control = (action: PlaybackAction) =>
    void run(
      "Управление плеером",
      () => call("control_playback", { session_id, action }),
      false,
    );
  const edit = async (action: QueueAction) => {
    if (!queue || queueLock.current) return;
    queueLock.current = true;
    setQueueBusy(true);
    setQueueError(false);
    try {
      const ok = await run(
        "Настройка очереди",
        async () => {
          await call("edit_queue", {
            session_id,
            queue_id: queue.queue_id,
            revision: queue.revision,
            action,
          });
          return true;
        },
        false,
      );
      if (!ok) setQueueError(true);
      // Events normally arrive first; refresh also reconciles a lost response/conflict.
      await runtime.refresh();
    } finally {
      queueLock.current = false;
      setQueueBusy(false);
    }
  };
  const toggle = () => {
    if (playing) control({ kind: "pause" });
    else if (track)
      void run(
        "Воспроизведение",
        () => resumePlayback(session_id, playback, queue),
        false,
      );
  };
  const open = (next: View = "artwork") => {
    setView(next);
    setFull(true);
  };
  const close = () => {
    if (view !== "artwork") setView("artwork");
    else setFull(false);
  };
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
  const transport = (
    <Transport
      playback={playback}
      queue={queue}
      busy={queueBusy}
      canPlay={!!track}
      control={control}
      edit={(action) => void edit(action)}
      toggle={toggle}
    />
  );
  const progress = (
    <SeekControl
      runtime={runtime}
      playback={playback}
      durationMs={durationMs}
    />
  );
  return (
    <>
      <footer
        className={`player ${!track ? "player-empty" : ""}`}
        aria-label="Аудиоплеер"
      >
        <div className="player-track">
          <button
            className="now-cover"
            aria-label="Открыть текущую композицию"
            disabled={!track}
            onClick={() => open()}
          >
            <Cover track={track} session={session_id} />
          </button>
          <button
            className="now-label"
            disabled={!track}
            onClick={() => open()}
          >
            <strong>{track?.metadata.title || "Выберите свою музыку"}</strong>
            <span>
              {playback.state === "failed"
                ? "Не удалось воспроизвести · повторить"
                : playback.state === "buffering"
                  ? "Подготовка аудио…"
                  : track?.metadata.performer || "Ваша личная коллекция"}
            </span>
          </button>
          <span className="desktop-like">{like}</span>
        </div>
        <div className="desktop-transport">
          {transport}
          {progress}
        </div>
        <div className="player-utilities">
          <IconButton
            label="Текст песни"
            disabled={!track}
            onClick={() => open("lyrics")}
          >
            <Mic2 />
          </IconButton>
          <IconButton
            label="Очередь воспроизведения"
            disabled={!queue?.total}
            onClick={() => open("queue")}
          >
            <ListMusic />
          </IconButton>
          <IconButton
            label="На весь экран"
            disabled={!track}
            onClick={() => open()}
          >
            <Maximize2 />
          </IconButton>
        </div>
        <div className="mobile-transport">
          <IconButton
            label={playing ? "Пауза" : "Слушать"}
            disabled={!track}
            onClick={toggle}
          >
            {playing ? (
              <Pause fill="currentColor" />
            ) : (
              <Play fill="currentColor" />
            )}
          </IconButton>
          <IconButton
            className="mini-next"
            label="Следующая композиция"
            disabled={!queue?.total}
            onClick={() => control({ kind: "next" })}
          >
            <SkipForward fill="currentColor" />
          </IconButton>
        </div>
        <progress
          className="mini-progress"
          aria-label="Прогресс воспроизведения"
          max={Math.max(1, durationMs)}
          value={playback.position_ms ?? 0}
        />
      </footer>
      {full && (
        <Modal
          title={
            view === "artwork"
              ? "Сейчас играет"
              : view === "lyrics"
                ? "Текст песни"
                : "Очередь"
          }
          fullScreen
          className={`player-dialog view-${view}`}
          onClose={close}
          headingAction={
            <IconButton
              label="Действия с композицией"
              disabled={!track}
              onClick={() => track && onMenu(track)}
            >
              <MoreHorizontal />
            </IconButton>
          }
        >
          <div className="now-playing-full">
            <div
              className="now-stage"
              onPointerDown={(e) => {
                if (view === "artwork" && e.pointerType === "touch")
                  gesture.current = { x: e.clientX, y: e.clientY };
              }}
              onPointerUp={(e) => {
                const start = gesture.current;
                gesture.current = null;
                if (
                  start &&
                  e.clientY - start.y > 80 &&
                  Math.abs(e.clientX - start.x) < 50
                )
                  setFull(false);
              }}
              onPointerCancel={() => {
                gesture.current = null;
              }}
            >
              {view === "artwork" ? (
                <div className="now-artwork" key={track?.track_id}>
                  <Cover track={track} session={session_id} />
                </div>
              ) : view === "lyrics" ? (
                <LyricsView
                  runtime={runtime}
                  track={track}
                  edit={() => track && onEdit(track)}
                />
              ) : (
                <QueuePanel runtime={runtime} />
              )}
            </div>
            <div className="now-controls">
              <div className="now-title">
                <div>
                  <h2>{track?.metadata.title || "Выберите композицию"}</h2>
                  <p>
                    {track?.metadata.performer || "Неизвестный исполнитель"}
                  </p>
                </div>
                {like}
              </div>
              {progress}
              {transport}
              {queueError && (
                <p role="alert" className="inline-error">
                  Не удалось изменить режим. Попробуйте ещё раз.
                </p>
              )}
              {playback.state === "failed" && (
                <p role="alert" className="inline-error">
                  Не удалось открыть аудио. Повторите воспроизведение. Если трек
                  не сохранён на устройстве, проверьте подключение.
                </p>
              )}
              <div
                className="now-tools"
                role="group"
                aria-label="Представление плеера"
              >
                {(
                  [
                    { id: "artwork", label: "Обложка", Icon: Disc3 },
                    { id: "lyrics", label: "Текст", Icon: Mic2 },
                    { id: "queue", label: "Очередь", Icon: ListMusic },
                  ] as const
                ).map(({ id, label, Icon }) => (
                  <button
                    key={id}
                    aria-pressed={view === id}
                    onClick={() => setView(id)}
                  >
                    <Icon />
                    {label}
                  </button>
                ))}
              </div>
              <p className="player-context">
                {queue?.position != null
                  ? `${queue.position + 1} из ${queue.total} в очереди`
                  : "Ваша музыка"}
              </p>
            </div>
          </div>
        </Modal>
      )}
    </>
  );
}
