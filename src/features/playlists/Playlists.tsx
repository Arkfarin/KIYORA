import { useEffect, useState } from "react";
import {
  ArrowDown,
  ArrowUp,
  ListMusic,
  Pencil,
  Play,
  Plus,
  Trash2,
} from "lucide-react";
import { call, errorOf, mutate, uuid } from "../../backend";
import type { Playlist, PlaylistDetail, Track } from "../../backend";
import type { Runtime } from "../../useBackend";
import type { Navigate } from "../../app/navigation";
import {
  Confirm,
  Empty,
  IconButton,
  Loading,
  Modal,
  PageHeading,
} from "../../components/ui/Primitives";
import { Cover } from "../../components/music/Cover";
export function PlaylistForm({
  runtime,
  playlist,
  onClose,
  onSaved,
}: {
  runtime: Runtime;
  playlist?: Playlist;
  onClose: () => void;
  onSaved: (id: string) => void;
}) {
  const [title, setTitle] = useState(playlist?.title ?? "");
  const [saving, setSaving] = useState(false);
  const [id] = useState(playlist?.playlist_id ?? uuid());
  const [op] = useState(uuid());
  return (
    <Modal
      title={playlist ? "Название плейлиста" : "Новый плейлист"}
      onClose={onClose}
    >
      <form
        className="stack"
        onSubmit={(e) => {
          e.preventDefault();
          if (saving) return;
          setSaving(true);
          void runtime
            .run("Сохранение плейлиста", async () => {
              await mutate(
                runtime.model.status!.context.session_id,
                id,
                {
                  action: playlist ? "playlist.rename" : "playlist.create",
                  payload: { title: title.trim() },
                },
                op,
              );
              onSaved(id);
            })
            .finally(() => setSaving(false));
        }}
      >
        <label>
          Название
          <input
            autoFocus
            value={title}
            maxLength={512}
            onChange={(e) => setTitle(e.target.value)}
            required
            placeholder="Музыка для…"
          />
        </label>
        <div className="dialog-actions">
          <button type="button" onClick={onClose}>
            Отмена
          </button>
          <button
            className="primary"
            disabled={
              saving ||
              !title.trim() ||
              runtime.model.status?.mode !== "ONLINE_READY"
            }
          >
            {saving ? "Сохраняем…" : "Сохранить"}
          </button>
        </div>
      </form>
    </Modal>
  );
}
export function PlaylistCover({
  runtime,
  playlist,
}: {
  runtime: Runtime;
  playlist: Playlist;
}) {
  const ids = [
    ...new Set(
      Object.values(runtime.model.projection?.state.entries ?? {})
        .filter((e) => e.playlist_id === playlist.playlist_id)
        .map((e) => e.track_id),
    ),
  ].slice(0, 4);
  return (
    <span className={`playlist-cover ${ids.length > 1 ? "mosaic" : ""}`}>
      {ids.length ? (
        ids.map((id) => (
          <Cover
            key={id}
            track={runtime.model.projection?.state.tracks[id]}
            session={runtime.model.status!.context.session_id}
          />
        ))
      ) : (
        <ListMusic size={42} strokeWidth={1} />
      )}
    </span>
  );
}
export function PlaylistsScreen({
  runtime,
  navigate,
  create,
}: {
  runtime: Runtime;
  navigate: Navigate;
  create: () => void;
}) {
  const playlists = Object.values(
    runtime.model.projection?.state.playlists ?? {},
  );
  return (
    <>
      <PageHeading title="Плейлисты" subtitle="Коллекции для ваших моментов">
        <button
          className="primary"
          disabled={runtime.model.status?.mode !== "ONLINE_READY"}
          onClick={create}
        >
          <Plus />
          Создать
        </button>
      </PageHeading>
      {!playlists.length ? (
        <Empty
          title="Музыка, собранная вами"
          action={
            <button
              onClick={create}
              disabled={runtime.model.status?.mode !== "ONLINE_READY"}
            >
              Создать первый плейлист
            </button>
          }
        >
          Добавляйте композиции из меню трека. Изменения синхронизируются через
          Vault.
        </Empty>
      ) : (
        <div className="album-grid">
          {playlists.map((p) => (
            <button
              key={p.playlist_id}
              className="album-card"
              onClick={() => navigate({ kind: "playlist", id: p.playlist_id })}
            >
              <PlaylistCover runtime={runtime} playlist={p} />
              <strong>{p.title}</strong>
              <span>Ваш плейлист</span>
            </button>
          ))}
        </div>
      )}
    </>
  );
}
export function PlaylistScreen({
  runtime,
  id,
  navigate,
  onMenu,
}: {
  runtime: Runtime;
  id: string;
  navigate: Navigate;
  onMenu: (t: Track) => void;
}) {
  const { model, run } = runtime;
  const session_id = model.status!.context.session_id;
  const online = model.status?.mode === "ONLINE_READY";
  const [detail, setDetail] = useState<PlaylistDetail | null>(null);
  const [offset, setOffset] = useState(0);
  const [error, setError] = useState("");
  const [rename, setRename] = useState(false);
  const [remove, setRemove] = useState(false);
  const [query, setQuery] = useState("");
  const [saving, setSaving] = useState(false);
  useEffect(() => {
    setOffset(0);
    setQuery("");
    setDetail(null);
  }, [id]);
  useEffect(() => {
    let active = true;
    setError("");
    void call("query", {
      session_id,
      query: {
        kind: "playlist",
        payload: { playlist_id: id, offset, limit: 100 },
      },
    })
      .then((r) => {
        if (active) setDetail(r.result as PlaylistDetail | null);
      })
      .catch((e) => {
        if (active) setError(errorOf(e).message);
      });
    return () => {
      active = false;
    };
  }, [id, offset, session_id, model.projection]);
  if (error) return <p role="alert">{error}</p>;
  if (!model.projection?.state.playlists[id])
    return (
      <Empty title="Плейлист недоступен">
        Он мог быть удалён на другом устройстве.
      </Empty>
    );
  if (!detail) return <Loading />;
  const items = detail.items.filter((i) =>
    `${i.track.metadata.title} ${i.track.metadata.performer}`
      .toLocaleLowerCase()
      .includes(query.toLocaleLowerCase()),
  );
  const change = (entry_id: string, after_entry_id: string | null) =>
    void run("Порядок плейлиста", () =>
      mutate(session_id, id, {
        action: "playlist.entry.move",
        payload: { entry_id, after_entry_id },
      }),
    );
  return (
    <>
      <PageHeading
        title={detail.playlist.title}
        subtitle={`${detail.total} композиций`}
      >
        <button
          className="primary"
          disabled={!detail.total}
          onClick={() =>
            void run(
              "Воспроизведение плейлиста",
              () =>
                call("queue_playlist", {
                  session_id,
                  playlist_id: id,
                  position: 0,
                  autoplay: true,
                }),
              false,
            )
          }
        >
          <Play fill="currentColor" />
          Слушать
        </button>
        <IconButton
          label="Переименовать плейлист"
          disabled={!online}
          onClick={() => setRename(true)}
        >
          <Pencil />
        </IconButton>
        <IconButton
          label="Удалить плейлист"
          disabled={!online}
          onClick={() => setRemove(true)}
        >
          <Trash2 />
        </IconButton>
      </PageHeading>
      <div className="collection-identity">
        <PlaylistCover runtime={runtime} playlist={detail.playlist} />
        <div>
          <p>Ваш плейлист</p>
          <button onClick={() => navigate({ kind: "library" })}>
            <Plus />
            Добавить музыку из медиатеки
          </button>
        </div>
      </div>
      <label className="search-field">
        <input
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder="Поиск на этой странице плейлиста"
          aria-label="Поиск на странице плейлиста"
        />
      </label>
      {!items.length ? (
        <Empty title={query ? "Совпадений нет" : "Пока пусто"}>
          Добавьте треки через меню композиции в медиатеке.
        </Empty>
      ) : (
        <ol className="playlist-entries">
          {items.map((item) => {
            const index = detail.items.indexOf(item);
            return (
              <li
                key={item.entry.entry_id}
                draggable={online && !query}
                onDragStart={(e) =>
                  e.dataTransfer.setData(
                    "application/x-kiyora-entry",
                    item.entry.entry_id,
                  )
                }
                onDragOver={(e) => {
                  if (online && !query) e.preventDefault();
                }}
                onDrop={(e) => {
                  e.preventDefault();
                  const source = e.dataTransfer.getData(
                    "application/x-kiyora-entry",
                  );
                  if (
                    source &&
                    source !== item.entry.entry_id &&
                    detail.items.some((i) => i.entry.entry_id === source)
                  )
                    change(source, item.entry.entry_id);
                }}
              >
                <button
                  className="queue-track"
                  onClick={() =>
                    void run(
                      "Воспроизведение",
                      () =>
                        call("queue_playlist", {
                          session_id,
                          playlist_id: id,
                          position: offset + index,
                          autoplay: true,
                        }),
                      false,
                    )
                  }
                >
                  <Cover session={session_id} track={item.track} />
                  <span>
                    <strong>{item.track.metadata.title}</strong>
                    <small>{item.track.metadata.performer}</small>
                  </span>
                </button>
                <div className="queue-actions">
                  <IconButton
                    label="Выше в плейлисте"
                    disabled={!online || index < 1 || !!query}
                    onClick={() =>
                      change(
                        item.entry.entry_id,
                        index > 1
                          ? detail.items[index - 2].entry.entry_id
                          : null,
                      )
                    }
                  >
                    <ArrowUp />
                  </IconButton>
                  <IconButton
                    label="Ниже в плейлисте"
                    disabled={
                      !online || index + 1 >= detail.items.length || !!query
                    }
                    onClick={() =>
                      change(
                        item.entry.entry_id,
                        detail.items[index + 1].entry.entry_id,
                      )
                    }
                  >
                    <ArrowDown />
                  </IconButton>
                  <IconButton
                    label="Убрать только из плейлиста"
                    disabled={!online}
                    onClick={() =>
                      void run("Удаление из плейлиста", () =>
                        mutate(session_id, id, {
                          action: "playlist.track.remove",
                          payload: { entry_id: item.entry.entry_id },
                        }),
                      )
                    }
                  >
                    <Trash2 />
                  </IconButton>
                  <button
                    onClick={() => onMenu(item.track)}
                    aria-label={`Действия с ${item.track.metadata.title}`}
                  >
                    Ещё
                  </button>
                </div>
              </li>
            );
          })}
        </ol>
      )}
      {detail.total > 100 && (
        <div className="pagination">
          <button disabled={!offset} onClick={() => setOffset((n) => n - 100)}>
            Назад
          </button>
          <span>
            {offset + 1}–{offset + detail.items.length} из {detail.total}
          </span>
          <button
            disabled={offset + 100 >= detail.total}
            onClick={() => setOffset((n) => n + 100)}
          >
            Далее
          </button>
        </div>
      )}
      {rename && (
        <PlaylistForm
          runtime={runtime}
          playlist={detail.playlist}
          onClose={() => setRename(false)}
          onSaved={() => setRename(false)}
        />
      )}
      {remove && (
        <Confirm
          title="Удалить плейлист?"
          confirmLabel="Удалить плейлист"
          busy={saving}
          onClose={() => setRemove(false)}
          onConfirm={() => {
            setSaving(true);
            void run("Удаление плейлиста", async () => {
              await mutate(session_id, id, {
                action: "playlist.delete",
                payload: {},
              });
              navigate({ kind: "playlists" });
            }).finally(() => setSaving(false));
          }}
        >
          Композиции останутся в медиатеке. Плейлист исчезнет на всех ваших
          устройствах.
        </Confirm>
      )}
    </>
  );
}
