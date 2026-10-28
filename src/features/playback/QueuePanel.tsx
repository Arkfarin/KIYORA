import { useEffect, useState } from "react";
import { ArrowDown, ArrowUp, Play, X } from "lucide-react";
import { call, errorOf } from "../../backend";
import type { QueueAction } from "../../backend";
import type { Runtime } from "../../useBackend";
import { Cover } from "../../components/music/Cover";
import { Empty, IconButton } from "../../components/ui/Primitives";
export function QueuePanel({ runtime }: { runtime: Runtime }) {
  const { model, run } = runtime;
  const session_id = model.status!.context.session_id;
  const [ids, setIds] = useState<string[]>([]);
  const [offset, setOffset] = useState(0);
  const [error, setError] = useState("");
  const [revision, setRevision] = useState(-1);
  useEffect(() => {
    let active = true;
    setError("");
    void call("playback_queue", { session_id, offset, limit: 100 })
      .then((page) => {
        if (active) {
          setIds(page.track_ids);
          setRevision(page.info.revision);
        }
      })
      .catch((e) => {
        if (active) setError(errorOf(e).message);
      });
    return () => {
      active = false;
    };
  }, [session_id, model.queue?.queue_id, model.queue?.revision, offset]);
  const queue = model.queue;
  const edit = (action: QueueAction) => {
    if (queue)
      void run(
        "Изменение очереди",
        () =>
          call("edit_queue", {
            session_id,
            queue_id: queue.queue_id,
            revision,
            action,
          }),
        false,
      );
  };
  return (
    <div className="queue-panel">
      <div className="section-heading">
        <h3>Очередь</h3>
        <span className="muted small">{queue?.total ?? 0} треков</span>
      </div>
      {error && <p role="alert">{error}</p>}
      {!queue?.total ? (
        <Empty title="Выберите, что слушать дальше">
          Добавляйте треки через меню композиции.
        </Empty>
      ) : (
        <>
          <ol className="queue-list" start={offset + 1}>
            {ids.map((id, i) => {
              const track = model.projection?.state.tracks[id];
              const index = offset + i;
              return (
                <li
                  key={`${index}:${id}`}
                  className={index === queue.position ? "queue-current" : ""}
                >
                  <button
                    className="queue-track"
                    onClick={() => edit({ kind: "jump", index })}
                    disabled={!track}
                  >
                    <Cover session={session_id} track={track} />
                    <span>
                      <strong>{track?.metadata.title ?? "Трек удалён"}</strong>
                      <small>
                        {index === queue.position
                          ? "Сейчас играет"
                          : track?.metadata.performer}
                      </small>
                    </span>
                    {index === queue.position && <Play size={14} />}
                  </button>
                  <div className="queue-actions">
                    <IconButton
                      label="Выше"
                      disabled={index === 0 || revision !== queue.revision}
                      onClick={() =>
                        edit({ kind: "move", from: index, to: index - 1 })
                      }
                    >
                      <ArrowUp />
                    </IconButton>
                    <IconButton
                      label="Ниже"
                      disabled={
                        index + 1 >= queue.total || revision !== queue.revision
                      }
                      onClick={() =>
                        edit({ kind: "move", from: index, to: index + 1 })
                      }
                    >
                      <ArrowDown />
                    </IconButton>
                    <IconButton
                      label="Убрать из очереди"
                      disabled={revision !== queue.revision}
                      onClick={() => edit({ kind: "remove", index })}
                    >
                      <X />
                    </IconButton>
                  </div>
                </li>
              );
            })}
          </ol>
          {queue.total > 100 && (
            <div className="pagination">
              <button
                disabled={!offset}
                onClick={() => setOffset((n) => Math.max(0, n - 100))}
              >
                Назад
              </button>
              <span>
                {offset + 1}–{offset + ids.length}
              </span>
              <button
                disabled={offset + 100 >= queue.total}
                onClick={() => setOffset((n) => n + 100)}
              >
                Далее
              </button>
            </div>
          )}
        </>
      )}
    </div>
  );
}
