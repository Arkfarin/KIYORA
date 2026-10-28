import { useEffect, useState } from "react";
import { HardDrive, Pin, RefreshCw, Trash2, X } from "lucide-react";
import { bytesLabel, call, errorOf, uuid } from "../../backend";
import type { CacheStats } from "../../backend";
import type { Runtime } from "../../useBackend";
import { Cover } from "../../components/music/Cover";
import { Empty, IconButton, PageHeading } from "../../components/ui/Primitives";
export function DownloadsScreen({ runtime }: { runtime: Runtime }) {
  const { model, run } = runtime;
  const session_id = model.status!.context.session_id;
  const [cached, setCached] = useState<
    { key: string; size_bytes: number; pinned: boolean }[]
  >([]);
  const [stats, setStats] = useState<CacheStats | null>(null);
  const [error, setError] = useState("");
  const [refresh, setRefresh] = useState(0);
  useEffect(() => {
    let active = true;
    void Promise.all([
      call("cached_media", { session_id }),
      call("vault", { session_id, request: { kind: "get_cache_stats" } }),
    ])
      .then(([entries, reply]) => {
        if (active) {
          setCached(
            entries.filter((e) => /^[0-9a-f-]{36}:[0-9a-f-]{36}$/.test(e.key)),
          );
          setStats(reply.result as CacheStats);
          setError("");
        }
      })
      .catch((e) => {
        if (active) setError(errorOf(e).message);
      });
    return () => {
      active = false;
    };
  }, [session_id, model.revision, refresh]);
  const downloads = Object.values(model.downloads).filter(
    (d) => d.state !== "fully_cached",
  );
  return (
    <>
      <PageHeading
        title="Загрузки"
        subtitle="Музыка, которая останется с вами без сети"
      >
        <IconButton
          label="Обновить загрузки"
          onClick={() => setRefresh((v) => v + 1)}
        >
          <RefreshCw />
        </IconButton>
      </PageHeading>
      {stats && (
        <div className="storage-summary">
          <HardDrive />
          <div>
            <strong>
              {bytesLabel(stats.used_bytes)}{" "}
              <span>из {bytesLabel(stats.quota_bytes)}</span>
            </strong>
            <progress
              max={stats.quota_bytes}
              value={stats.used_bytes}
              aria-label="Использование кэша"
            />
            <p>
              {stats.pinned_entries} закреплённых файлов ·{" "}
              {stats.partial_entries} частичных загрузок
            </p>
          </div>
        </div>
      )}
      {error && <p role="alert">{error}</p>}
      {!!downloads.length && (
        <section>
          <h2>Активность</h2>
          {downloads.map((d) => {
            const track = model.projection?.state.tracks[d.track_id];
            const source =
              track &&
              ((track.preferred_source_id
                ? track.sources[track.preferred_source_id]
                : undefined) ??
                Object.values(track.sources)[0]);
            return (
              <div className="download-row" key={d.download_id}>
                <Cover session={session_id} track={track} />
                <div>
                  <strong>{track?.metadata.title || "Композиция"}</strong>
                  <p>
                    {d.state === "downloading"
                      ? "Скачиваем"
                      : d.state === "cancelled"
                        ? "Отменено"
                        : d.state === "failed_permanent"
                          ? "Источник недоступен"
                          : "Загрузка не завершена"}{" "}
                    · {bytesLabel(d.bytes_downloaded)}
                    {d.total_bytes ? ` / ${bytesLabel(d.total_bytes)}` : ""}
                  </p>
                  {d.total_bytes && (
                    <progress
                      max={d.total_bytes}
                      value={d.bytes_downloaded}
                      aria-label="Прогресс загрузки"
                    />
                  )}
                </div>
                {d.state === "downloading" && (
                  <IconButton
                    label="Отменить скачивание"
                    onClick={() =>
                      void run("Отмена скачивания", () =>
                        call("vault", {
                          session_id,
                          request: {
                            kind: "cancel_download",
                            payload: { download_id: d.download_id },
                          },
                        }),
                      )
                    }
                  >
                    <X />
                  </IconButton>
                )}
                {source &&
                  [
                    "cancelled",
                    "failed_retryable",
                    "partially_cached",
                  ].includes(d.state) && (
                    <button
                      disabled={
                        model.status?.mode !== "ONLINE_READY" || !!runtime.busy
                      }
                      onClick={() =>
                        void run("Продолжить скачивание", () =>
                          call("vault", {
                            session_id,
                            request: {
                              kind: "download_for_offline",
                              payload: {
                                download_id: uuid(),
                                track_id: d.track_id,
                                source_id: source.source_id,
                                pinned: d.pinned,
                              },
                            },
                          }),
                        )
                      }
                    >
                      Продолжить
                    </button>
                  )}
              </div>
            );
          })}
        </section>
      )}
      <section>
        <h2>Сохранено на устройстве</h2>
        {!cached.length ? (
          <Empty title="Музыка ещё не сохранена">
            Откройте меню композиции и выберите «Скачать для офлайна».
          </Empty>
        ) : (
          cached.map((entry) => {
            const [track_id, source_id] = entry.key.split(":");
            const track = model.projection?.state.tracks[track_id];
            return (
              <div className="download-row" key={entry.key}>
                <Cover session={session_id} track={track} />
                <div>
                  <strong>
                    {track?.metadata.title || "Копия удалённой композиции"}
                  </strong>
                  <p>
                    {bytesLabel(entry.size_bytes)} ·{" "}
                    {entry.pinned
                      ? "Офлайн, закреплено"
                      : "Кэш воспроизведения"}
                  </p>
                </div>
                <IconButton
                  label={
                    entry.pinned
                      ? "Разрешить автоматическую очистку"
                      : "Закрепить для офлайна"
                  }
                  aria-pressed={entry.pinned}
                  onClick={() =>
                    void run("Изменение закрепления", async () => {
                      await call("vault", {
                        session_id,
                        request: {
                          kind: "pin_cached_media",
                          payload: {
                            track_id,
                            source_id,
                            pinned: !entry.pinned,
                          },
                        },
                      });
                      setRefresh((v) => v + 1);
                    })
                  }
                >
                  <Pin />
                </IconButton>
                <IconButton
                  label="Удалить только локальную копию"
                  disabled={model.playback.track_id === track_id}
                  onClick={() =>
                    void run("Удаление локальной копии", async () => {
                      await call("vault", {
                        session_id,
                        request: {
                          kind: "remove_cached_media",
                          payload: { track_id, source_id },
                        },
                      });
                      setRefresh((v) => v + 1);
                    })
                  }
                >
                  <Trash2 />
                </IconButton>
              </div>
            );
          })
        )}
      </section>
      <p className="muted small">
        Удаление локальной копии не удаляет музыку из Vault. Файлы, используемые
        плеером, защищены от очистки.
      </p>
    </>
  );
}
