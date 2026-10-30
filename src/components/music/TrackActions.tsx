import { useRef, useState } from "react";
import {
  Disc3,
  Download,
  Heart,
  ListEnd,
  ListPlus,
  Mic2,
  Pencil,
  Play,
  Share2,
  SkipForward,
  Trash2,
} from "lucide-react";
import { call, mutate, uuid } from "../../backend";
import type { Track } from "../../backend";
import type { Runtime } from "../../useBackend";
import type { Navigate } from "../../app/navigation";
import { Cover } from "./Cover";
import { Modal } from "../ui/Primitives";
export function TrackActions({
  runtime,
  track,
  onClose,
  onEdit,
  onDelete,
  navigate,
}: {
  runtime: Runtime;
  track: Track;
  onClose: () => void;
  onEdit: (track: Track) => void;
  onDelete: (tracks: Track[]) => void;
  navigate: Navigate;
}) {
  const { model, run } = runtime;
  const session_id = model.status!.context.session_id;
  const online = model.status?.mode === "ONLINE_READY";
  const [view, setView] = useState<"menu" | "playlist" | "share">("menu");
  const [recipient, setRecipient] = useState("");
  const [sending, setSending] = useState(false);
  const [shareState, setShareState] = useState("");
  const shareOp = useRef(uuid());
  const liked = model.projection?.state.favorites.liked?.includes(
    track.track_id,
  );
  const source =
    (track.preferred_source_id
      ? track.sources[track.preferred_source_id]
      : undefined) ?? Object.values(track.sources)[0];
  const queue = model.queue;
  const enqueue = (next: boolean) => {
    if (!queue) return;
    void run(
      "Добавление в очередь",
      async () => {
        await call("edit_queue", {
          session_id,
          queue_id: queue.queue_id,
          revision: queue.revision,
          action: { kind: "append", track_ids: [track.track_id], next },
        });
        onClose();
      },
      false,
    );
  };
  return (
    <Modal
      title={
        view === "playlist"
          ? "Добавить в плейлист"
          : view === "share"
            ? "Поделиться композицией"
            : "Композиция"
      }
      onClose={() => (view === "menu" ? onClose() : setView("menu"))}
    >
      <div className="menu-track">
        <Cover track={track} session={session_id} />
        <div>
          <strong>{track.metadata.title}</strong>
          <p>{track.metadata.performer || "Неизвестный исполнитель"}</p>
        </div>
      </div>
      {view === "menu" && (
        <div className="action-menu">
          <button
            onClick={() =>
              void run(
                "Воспроизведение",
                async () => {
                  await call("set_queue", {
                    session_id,
                    track_ids: [track.track_id],
                    position: 0,
                    autoplay: true,
                  });
                  onClose();
                },
                false,
              )
            }
          >
            <Play />
            Воспроизвести
          </button>
          <button disabled={!queue} onClick={() => enqueue(true)}>
            <SkipForward />
            Следующий в очереди
          </button>
          <button disabled={!queue} onClick={() => enqueue(false)}>
            <ListEnd />
            Добавить в очередь
          </button>
          <button disabled={!online} onClick={() => setView("playlist")}>
            <ListPlus />
            Добавить в плейлист
          </button>
          <button
            disabled={!online}
            onClick={() =>
              void run("Избранное", async () => {
                await mutate(session_id, track.track_id, {
                  action: "favorite.set",
                  payload: { set_name: "liked", value: !liked },
                });
                onClose();
              })
            }
          >
            <Heart fill={liked ? "currentColor" : "none"} />
            {liked ? "Убрать из любимых" : "Добавить в любимые"}
          </button>
          {track.metadata.album && (
            <button
              onClick={() => {
                navigate({
                  kind: "album",
                  title: track.metadata.album,
                  artist:
                    track.metadata.album_artist || track.metadata.performer,
                });
                onClose();
              }}
            >
              <Disc3 />
              Открыть альбом
            </button>
          )}
          {track.metadata.performer && (
            <button
              onClick={() => {
                navigate({ kind: "artist", name: track.metadata.performer });
                onClose();
              }}
            >
              <Mic2 />
              Открыть исполнителя
            </button>
          )}
          <button
            onClick={() => {
              onEdit(track);
              onClose();
            }}
          >
            <Pencil />
            Изменить информацию
          </button>
          <button
            disabled={
              !online ||
              !source ||
              model.downloads[track.track_id]?.state === "downloading"
            }
            onClick={() =>
              void run("Загрузка для офлайна", async () => {
                await call("start_job", {
                  session_id,
                  job_id: uuid(),
                  request: {
                    kind: "vault",
                    payload: {
                      kind: "download_for_offline",
                      payload: {
                        download_id: uuid(),
                        track_id: track.track_id,
                        source_id: source.source_id,
                        pinned: true,
                      },
                    },
                  },
                });
                onClose();
              })
            }
          >
            <Download />
            Скачать для офлайна
          </button>
          <button
            disabled={!online || !source}
            onClick={() => setView("share")}
          >
            <Share2 />
            Поделиться
          </button>
          <button
            className="danger-text"
            disabled={!online}
            onClick={() => {
              onDelete([track]);
              onClose();
            }}
          >
            <Trash2 />
            Удалить из библиотеки
          </button>
        </div>
      )}
      {view === "playlist" && (
        <div className="action-menu">
          {Object.values(model.projection?.state.playlists ?? {}).map((p) => (
            <button
              key={p.playlist_id}
              disabled={!online || sending}
              onClick={() => {
                setSending(true);
                void run("Добавление в плейлист", async () => {
                  await mutate(session_id, p.playlist_id, {
                    action: "playlist.track.add",
                    payload: {
                      track_id: track.track_id,
                      entry_id: uuid(),
                      after_entry_id: null,
                    },
                  });
                  onClose();
                }).finally(() => setSending(false));
              }}
            >
              <ListPlus />
              {p.title}
            </button>
          ))}
          {!Object.keys(model.projection?.state.playlists ?? {}).length && (
            <p>Сначала создайте плейлист в разделе «Плейлисты».</p>
          )}
          <button onClick={() => setView("menu")}>Назад</button>
        </div>
      )}
      {view === "share" && (
        <form
          className="stack"
          onSubmit={(e) => {
            e.preventDefault();
            if (sending || shareState) return;
            setSending(true);
            void run("Отправка композиции", async () => {
              const reply = await call("vault", {
                session_id,
                request: {
                  kind: "share_track",
                  payload: {
                    op_id: shareOp.current,
                    track_id: track.track_id,
                    source_id: source.source_id,
                    recipient_chat_id: recipient,
                  },
                },
              });
              const result = reply.result as { state: string };
              setShareState(
                result.state === "sent"
                  ? "Композиция отправлена."
                  : "Отправка не подтверждена. Не повторяйте её: проверьте состояние в Telegram.",
              );
            }).finally(() => setSending(false));
          }}
        >
          <p>Композиция отправится в выбранный Telegram-чат от вашего имени.</p>
          <label>
            Chat ID получателя
            <input
              value={recipient}
              onChange={(e) => setRecipient(e.target.value.trim())}
              required
              pattern="-?[1-9][0-9]*"
              placeholder="Числовой ID чата"
            />
          </label>
          {shareState && <p role="status">{shareState}</p>}
          <button
            className="primary"
            disabled={
              !online ||
              sending ||
              !!shareState ||
              !/^-?[1-9]\d*$/.test(recipient)
            }
          >
            {sending ? "Отправляем…" : "Отправить в Telegram"}
          </button>
        </form>
      )}
    </Modal>
  );
}
