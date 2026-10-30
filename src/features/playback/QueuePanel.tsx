import { memo, useEffect, useRef, useState } from "react";
import { ArrowDown, ArrowUp, Play, X } from "lucide-react";
import { call } from "../../backend";
import type { QueueAction, Queue } from "../../backend";
import type { Runtime } from "../../useBackend";
import { Cover } from "../../components/music/Cover";
import { Empty, IconButton, Loading } from "../../components/ui/Primitives";

export const QueuePanel = memo(function QueuePanel({
  runtime,
}: {
  runtime: Runtime;
}) {
  const { model } = runtime;
  const session_id = model.status!.context.session_id;
  const queue = model.queue;
  const [page, setPage] = useState<{
    ids: string[];
    info: Queue;
    offset: number;
  }>();
  const [offset, setOffset] = useState(
    () => Math.floor((queue?.position ?? 0) / 100) * 100,
  );
  const [error, setError] = useState(false);
  const [editError, setEditError] = useState(false);
  const [loading, setLoading] = useState(true);
  const [retry, setRetry] = useState(0);
  const [busy, setBusy] = useState(false);
  const lock = useRef(false);
  useEffect(() => {
    setOffset(Math.floor((queue?.position ?? 0) / 100) * 100);
  }, [queue?.queue_id]);
  useEffect(() => {
    if (offset >= (queue?.total ?? 0))
      setOffset(Math.max(0, Math.ceil((queue?.total ?? 0) / 100) - 1) * 100);
  }, [offset, queue?.total]);
  useEffect(() => {
    let active = true;
    setError(false);
    setLoading(true);
    void call("playback_queue", { session_id, offset, limit: 100 })
      .then((value) => {
        if (active) setPage({ ids: value.track_ids, info: value.info, offset });
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
  }, [session_id, queue?.queue_id, queue?.revision, offset, retry]);
  const fresh =
    !loading &&
    !error &&
    page?.info.queue_id === queue?.queue_id &&
    page?.info.revision === queue?.revision &&
    page?.offset === offset;
  const edit = async (action: QueueAction) => {
    if (!queue || !page || !fresh || lock.current) return;
    lock.current = true;
    setBusy(true);
    setEditError(false);
    try {
      const ok = await runtime.run(
        "Изменение очереди",
        async () => {
          await call("edit_queue", {
            session_id,
            queue_id: page.info.queue_id,
            revision: page.info.revision,
            action,
          });
          return true;
        },
        false,
      );
      if (!ok) setEditError(true);
      await runtime.refresh();
      setRetry((n) => n + 1);
    } finally {
      lock.current = false;
      setBusy(false);
    }
  };
  return (
    <div className="queue-panel">
      <div className="section-heading">
        <h3>Далее</h3>
        <span className="muted small">{queue?.total ?? 0} треков</span>
      </div>
      <p className="queue-mode">
        {queue?.shuffle ? "Случайный порядок" : "По порядку"} ·{" "}
        {queue?.repeat === "track"
          ? "Повтор трека"
          : queue?.repeat === "queue"
            ? "Повтор очереди"
            : "Остановка в конце"}
      </p>
      {error && (
        <div role="alert" className="inline-error">
          Не удалось обновить очередь.{" "}
          <button onClick={() => setRetry((n) => n + 1)}>Повторить</button>
        </div>
      )}
      {editError && (
        <p role="alert" className="inline-error">
          Изменение не подтверждено. Очередь обновлена; попробуйте действие ещё
          раз.
        </p>
      )}
      {!queue?.total ? (
        <Empty title="Что послушаем дальше?">
          Добавляйте треки через меню композиции.
        </Empty>
      ) : (
        <>
          {loading && !page && <Loading label="Открываем очередь…" />}
          <ol
            className="queue-list"
            start={offset + 1}
            aria-busy={loading || busy}
          >
            {page?.ids.map((id, i) => {
              const track = model.projection?.state.tracks[id];
              const index = page.offset + i;
              const current = index === queue.position;
              return (
                <li
                  key={`${index}:${id}`}
                  className={current ? "queue-current" : ""}
                  aria-current={current ? "true" : undefined}
                >
                  <button
                    className="queue-track"
                    onClick={() => void edit({ kind: "jump", index })}
                    disabled={!track || !fresh || busy}
                  >
                    <Cover session={session_id} track={track} />
                    <span>
                      <strong>{track?.metadata.title ?? "Трек удалён"}</strong>
                      <small>
                        {current ? "Сейчас играет" : track?.metadata.performer}
                      </small>
                    </span>
                    {current && <Play size={14} />}
                  </button>
                  <div className="queue-actions">
                    <IconButton
                      label="Выше"
                      disabled={!index || !fresh || busy}
                      onClick={() =>
                        void edit({ kind: "move", from: index, to: index - 1 })
                      }
                    >
                      <ArrowUp />
                    </IconButton>
                    <IconButton
                      label="Ниже"
                      disabled={index + 1 >= queue.total || !fresh || busy}
                      onClick={() =>
                        void edit({ kind: "move", from: index, to: index + 1 })
                      }
                    >
                      <ArrowDown />
                    </IconButton>
                    <IconButton
                      label="Убрать из очереди"
                      disabled={!fresh || busy}
                      onClick={() => void edit({ kind: "remove", index })}
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
                disabled={!offset || busy}
                onClick={() => setOffset((n) => Math.max(0, n - 100))}
              >
                Назад
              </button>
              <span>
                {offset + 1}–{offset + (page?.ids.length ?? 0)}
              </span>
              <button
                disabled={offset + 100 >= queue.total || busy}
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
});
