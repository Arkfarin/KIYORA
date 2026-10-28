import { useEffect, useMemo, useRef, useState } from "react";
import {
  Check,
  Cloud,
  Download,
  Heart,
  LayoutGrid,
  List,
  MoreHorizontal,
  Pause,
  Play,
  Search,
  Trash2,
  X,
} from "lucide-react";
import { call, duration, mutate } from "../../backend";
import type { Track } from "../../backend";
import type { Runtime } from "../../useBackend";
import type { Navigate } from "../../app/navigation";
import { Cover } from "./Cover";
import { Empty, IconButton } from "../ui/Primitives";
type Sort = "title" | "performer" | "album" | "recent";
export function sortTracks(tracks: Track[], sort: Sort) {
  return [...tracks].sort((a, b) =>
    sort === "recent"
      ? Number(BigInt(b.updated_order) - BigInt(a.updated_order))
      : a.metadata[sort].localeCompare(b.metadata[sort], undefined, {
          sensitivity: "base",
          numeric: true,
        }),
  );
}
export function TrackList({
  tracks,
  runtime,
  navigate,
  onMenu,
  onDelete,
  compact = false,
  playIds,
}: {
  tracks: Track[];
  runtime: Runtime;
  navigate: Navigate;
  onMenu: (track: Track) => void;
  onDelete: (tracks: Track[]) => void;
  compact?: boolean;
  playIds?: string[];
}) {
  const { model, run } = runtime;
  const session_id = model.status!.context.session_id;
  const [query, setQuery] = useState("");
  const [sort, setSort] = useState<Sort>("title");
  const [grid, setGrid] = useState(false);
  const [page, setPage] = useState(0);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const press = useRef<ReturnType<typeof setTimeout> | null>(null);
  const list = useMemo(
    () =>
      compact
        ? tracks
        : sortTracks(
            tracks.filter((t) =>
              `${t.metadata.title} ${t.metadata.performer} ${t.metadata.album}`
                .toLocaleLowerCase()
                .includes(query.toLocaleLowerCase()),
            ),
            sort,
          ),
    [tracks, sort, query, compact],
  );
  const maxPage = Math.max(0, Math.ceil(list.length / 50) - 1);
  const currentPage = Math.min(page, maxPage);
  const visible = compact
    ? list
    : list.slice(currentPage * 50, (currentPage + 1) * 50);
  const favorites = useMemo(
    () => new Set(model.projection?.state.favorites.liked ?? []),
    [model.projection],
  );
  const stopPress = () => {
    if (press.current) clearTimeout(press.current);
    press.current = null;
  };
  useEffect(() => () => stopPress(), []);
  useEffect(() => {
    setPage(0);
    setSelected(new Set());
  }, [query, sort, session_id]);
  useEffect(() => {
    setSelected(
      (old) =>
        new Set([...old].filter((id) => tracks.some((t) => t.track_id === id))),
    );
  }, [tracks]);
  const play = (track: Track) => {
    const ids = playIds ?? list.map((t) => t.track_id);
    void run(
      "Воспроизведение",
      () =>
        call("set_queue", {
          session_id,
          track_ids: ids,
          position: ids.indexOf(track.track_id),
          autoplay: true,
        }),
      false,
    );
  };
  const toggle = (id: string) =>
    setSelected((old) => {
      const next = new Set(old);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  return (
    <div
      className="track-collection"
      onKeyDown={(e) => {
        if (
          e.key === "Delete" &&
          selected.size &&
          !(e.target instanceof HTMLInputElement)
        ) {
          e.preventDefault();
          onDelete(tracks.filter((t) => selected.has(t.track_id)));
        }
      }}
    >
      {!compact && (
        <div className="collection-toolbar">
          <label className="search-field">
            <Search size={18} />
            <input
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder="Поиск в коллекции"
              aria-label="Поиск в коллекции"
            />
          </label>
          <div className="actions">
            <label className="sort-select">
              <span className="sr-only">Сортировка</span>
              <select
                value={sort}
                onChange={(e) => setSort(e.target.value as Sort)}
              >
                <option value="title">По названию</option>
                <option value="performer">По исполнителю</option>
                <option value="album">По альбому</option>
                <option value="recent">Последние изменения</option>
              </select>
            </label>
            <IconButton
              label={grid ? "Показать список" : "Показать карточки"}
              onClick={() => setGrid((v) => !v)}
            >
              {grid ? <List /> : <LayoutGrid />}
            </IconButton>
          </div>
        </div>
      )}
      {!!selected.size && (
        <div className="selection-bar">
          <span>Выбрано: {selected.size}</span>
          <button
            onClick={() =>
              void run(
                "Добавление в очередь",
                () =>
                  call("edit_queue", {
                    session_id,
                    queue_id: model.queue!.queue_id,
                    revision: model.queue!.revision,
                    action: {
                      kind: "append",
                      track_ids: tracks
                        .filter((t) => selected.has(t.track_id))
                        .map((t) => t.track_id),
                      next: false,
                    },
                  }),
                false,
              )
            }
            disabled={!model.queue}
          >
            В очередь
          </button>
          <IconButton
            label="Удалить выбранные из библиотеки"
            disabled={model.status?.mode !== "ONLINE_READY"}
            onClick={() =>
              onDelete(tracks.filter((t) => selected.has(t.track_id)))
            }
          >
            <Trash2 />
          </IconButton>
          <IconButton
            label="Снять выделение"
            onClick={() => setSelected(new Set())}
          >
            <X />
          </IconButton>
        </div>
      )}
      {!visible.length ? (
        <Empty title={query ? "Ничего не найдено" : "Пока ни одного трека"}>
          {query
            ? "Попробуйте другое название или исполнителя."
            : "Добавьте свою музыку, чтобы начать слушать."}
        </Empty>
      ) : (
        <>
          {!grid && !compact && (
            <div className="track-table-heading">
              <label>
                <input
                  type="checkbox"
                  aria-label="Выбрать треки на странице"
                  checked={
                    visible.length > 0 &&
                    visible.every((t) => selected.has(t.track_id))
                  }
                  onChange={(e) =>
                    setSelected((old) => {
                      const next = new Set(old);
                      visible.forEach((t) =>
                        e.target.checked
                          ? next.add(t.track_id)
                          : next.delete(t.track_id),
                      );
                      return next;
                    })
                  }
                />
              </label>
              <span>Композиция</span>
              <span>Альбом</span>
              <span>Формат</span>
              <span>Время</span>
              <span />
            </div>
          )}
          <div className={grid ? "music-grid" : "track-list"}>
            {visible.map((track) => {
              const playing =
                model.playback.track_id === track.track_id &&
                model.playback.state === "playing";
              const cache = model.downloads[track.track_id];
              const source = track.preferred_source_id ?? Object.keys(track.sources)[0];
              const cached = !!model.cached[`${track.track_id}:${source}`];
              return (
                <div
                  className={`${grid ? "music-card" : "track-row"} ${playing ? "is-playing" : ""} ${selected.has(track.track_id) ? "is-selected" : ""} ${compact ? "compact-row" : ""}`}
                  key={track.track_id}
                  onDoubleClick={(e) => {
                    if (!(e.target as HTMLElement).closest("button,input"))
                      play(track);
                  }}
                  onContextMenu={(e) => {
                    e.preventDefault();
                    onMenu(track);
                  }}
                  onPointerDown={(e) => {
                    if (e.pointerType === "touch")
                      press.current = setTimeout(() => onMenu(track), 550);
                  }}
                  onPointerUp={stopPress}
                  onPointerMove={stopPress}
                  onPointerCancel={stopPress}
                >
                  {!compact && !grid && (
                    <label className="row-select">
                      <input
                        type="checkbox"
                        aria-label={`Выбрать ${track.metadata.title}`}
                        checked={selected.has(track.track_id)}
                        onChange={() => toggle(track.track_id)}
                      />
                    </label>
                  )}
                  <div className="track-identity">
                    <button
                      className="cover-play"
                      aria-label={`Слушать ${track.metadata.title}`}
                      onClick={() =>
                        playing
                          ? void run(
                              "Пауза",
                              () =>
                                call("control_playback", {
                                  session_id,
                                  action: { kind: "pause" },
                                }),
                              false,
                            )
                          : play(track)
                      }
                    >
                      <Cover track={track} session={session_id} />
                      <span>
                        {playing ? (
                          <Pause fill="currentColor" />
                        ) : (
                          <Play fill="currentColor" />
                        )}
                      </span>
                    </button>
                    <div className="track-label">
                      <button
                        className="track-title"
                        onClick={() => play(track)}
                        title={track.metadata.title}
                      >
                        {track.metadata.title || "Без названия"}
                      </button>
                      <button
                        className="track-artist"
                        onClick={() =>
                          track.metadata.performer &&
                          navigate({
                            kind: "artist",
                            name: track.metadata.performer,
                          })
                        }
                        disabled={!track.metadata.performer}
                      >
                        {track.metadata.performer || "Неизвестный исполнитель"}
                      </button>
                    </div>
                  </div>
                  {!compact && !grid && (
                    <button
                      className="track-album text-button"
                      disabled={!track.metadata.album}
                      onClick={() =>
                        navigate({
                          kind: "album",
                          title: track.metadata.album,
                          artist:
                            track.metadata.album_artist ||
                            track.metadata.performer,
                        })
                      }
                    >
                      {track.metadata.album || "—"}
                    </button>
                  )}
                  {!compact && !grid && (
                    <span className="track-format">
                      {track.metadata.codec?.toUpperCase() || "—"}
                      {cached ? (
                        <Check size={14} aria-label="Сохранён" />
                      ) : cache?.state === "downloading" ? (
                        <Download size={14} aria-label="Скачивается" />
                      ) : (
                        <Cloud size={14} aria-label="В облаке" />
                      )}
                    </span>
                  )}
                  <span className="track-duration">
                    {duration(track.metadata.duration_ms)}
                  </span>
                  <div className="track-actions">
                    <IconButton
                      label={
                        favorites.has(track.track_id)
                          ? "Убрать из любимых"
                          : "В любимые"
                      }
                      className={`favorite-button ${favorites.has(track.track_id) ? "is-liked" : ""}`}
                      disabled={model.status?.mode !== "ONLINE_READY"}
                      onClick={() =>
                        void run("Избранное", () =>
                          mutate(session_id, track.track_id, {
                            action: "favorite.set",
                            payload: {
                              set_name: "liked",
                              value: !favorites.has(track.track_id),
                            },
                          }),
                        )
                      }
                    >
                      <Heart
                        fill={
                          favorites.has(track.track_id)
                            ? "currentColor"
                            : "none"
                        }
                      />
                    </IconButton>
                    <IconButton
                      label={`Действия: ${track.metadata.title}`}
                      onClick={() => onMenu(track)}
                    >
                      <MoreHorizontal />
                    </IconButton>
                  </div>
                </div>
              );
            })}
          </div>
        </>
      )}
      {!compact && list.length > 50 && (
        <div className="pagination">
          <button
            disabled={!currentPage}
            onClick={() => setPage(currentPage - 1)}
          >
            Назад
          </button>
          <span>
            {currentPage * 50 + 1}–
            {Math.min((currentPage + 1) * 50, list.length)} из {list.length}
          </span>
          <button
            disabled={currentPage >= maxPage}
            onClick={() => setPage(currentPage + 1)}
          >
            Далее
          </button>
        </div>
      )}
    </div>
  );
}
