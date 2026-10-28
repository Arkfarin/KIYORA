import { useEffect, useMemo, useState } from "react";
import { ArrowRight, Heart, ListMusic, Play, Plus, Search } from "lucide-react";
import type { Runtime } from "../../useBackend";
import { call, duration, errorOf } from "../../backend";
import type { Track } from "../../backend";
import type { Navigate, Route } from "../../app/navigation";
import { PageHeading, Empty, Loading } from "../../components/ui/Primitives";
import { resumePlayback } from "../../services/playback";
import { Cover } from "../../components/music/Cover";
import { TrackList } from "../../components/music/TrackList";
type Props = {
  runtime: Runtime;
  navigate: Navigate;
  onMenu: (track: Track) => void;
  onDelete: (tracks: Track[]) => void;
};
export function useCollection(runtime: Runtime) {
  return useMemo(() => {
    const tracks = Object.values(runtime.model.projection?.state.tracks ?? {});
    const albums = new Map<
      string,
      { title: string; artist: string; tracks: Track[] }
    >();
    const artists = new Map<string, Track[]>();
    for (const track of tracks) {
      const { album, album_artist, performer } = track.metadata;
      if (album) {
        const key = JSON.stringify([album, album_artist || performer]);
        const item = albums.get(key) ?? {
          title: album,
          artist: album_artist || performer,
          tracks: [],
        };
        item.tracks.push(track);
        albums.set(key, item);
      }
      if (performer) {
        const items = artists.get(performer) ?? [];
        items.push(track);
        artists.set(performer, items);
      }
    }
    return {
      tracks,
      albums: [...albums.values()],
      artists: [...artists].map(([name, tracks]) => ({ name, tracks })),
    };
  }, [runtime.model.projection]);
}
export function HomeScreen(props: Props) {
  const { runtime, navigate } = props;
  const { model, run } = runtime;
  const { tracks, albums } = useCollection(runtime);
  const session_id = model.status!.context.session_id;
  const favorites = model.projection?.state.favorites.liked ?? [];
  const playlists = Object.values(model.projection?.state.playlists ?? {});
  const recent = useMemo(
    () =>
      [...tracks]
        .sort((a, b) => {
          const order = (t: Track) =>
            Object.values(t.sources).reduce(
              (n, s) => (BigInt(s.message_id) < n ? BigInt(s.message_id) : n),
              9223372036854775807n,
            );
          return Number(order(b) - order(a));
        })
        .slice(0, 6),
    [tracks],
  );
  const current = tracks.find(
    (t) => t.track_id === model.queue?.current_track_id,
  );
  return (
    <>
      <PageHeading
        title="Ваша музыка"
        subtitle="Всё, что хочется слушать. Всегда с вами."
      >
        <button
          className="primary"
          disabled={model.status?.mode !== "ONLINE_READY"}
          onClick={() => navigate({ kind: "imports" })}
        >
          <Plus />
          Добавить музыку
        </button>
      </PageHeading>
      {!tracks.length ? (
        <div className="first-library">
          <div>
            <h2>
              Здесь начинается
              <br />
              ваша коллекция.
            </h2>
            <p>
              Загрузите любимый альбом или несколько композиций. Музыка
              сохранится в вашем приватном Telegram Vault.
            </p>
            <button
              onClick={() => navigate({ kind: "imports" })}
              disabled={model.status?.mode !== "ONLINE_READY"}
            >
              Выбрать файлы
              <ArrowRight />
            </button>
          </div>
          <div className="empty-record" aria-hidden="true">
            <span>KIYORA</span>
            <span>
              Ваша музыка.
              <br />
              Ваше облако.
            </span>
          </div>
        </div>
      ) : (
        <>
          <div className="home-quick-links">
            <button onClick={() => navigate({ kind: "favorites" })}>
              <Heart />
              <span>
                <strong>Любимые треки</strong>
                <small>{favorites.length} композиций</small>
              </span>
              <ArrowRight />
            </button>
            {current ? (
              <button
                onClick={() =>
                  void run(
                    "Продолжить прослушивание",
                    () =>
                      resumePlayback(session_id, model.playback, model.queue),
                    false,
                  )
                }
              >
                <Cover track={current} session={session_id} />
                <span>
                  <strong>{current.metadata.title}</strong>
                  <small>Продолжить прослушивание</small>
                </span>
                <Play />
              </button>
            ) : (
              <button onClick={() => navigate({ kind: "library" })}>
                <ListMusic />
                <span>
                  <strong>Вся коллекция</strong>
                  <small>{tracks.length} композиций</small>
                </span>
                <ArrowRight />
              </button>
            )}
          </div>
          <section>
            <div className="section-heading">
              <h2>Недавно добавленные</h2>
              <button
                className="text-button"
                onClick={() => navigate({ kind: "library" })}
              >
                Все треки
                <ArrowRight size={16} />
              </button>
            </div>
            <TrackList {...props} tracks={recent} compact />
          </section>
          {!!albums.length && (
            <section>
              <div className="section-heading">
                <h2>Ваши альбомы</h2>
                <button
                  className="text-button"
                  onClick={() => navigate({ kind: "albums" })}
                >
                  Все альбомы
                  <ArrowRight size={16} />
                </button>
              </div>
              <div className="album-grid">
                {albums.slice(0, 6).map((a) => (
                  <button
                    className="album-card"
                    key={JSON.stringify([a.title, a.artist])}
                    onClick={() =>
                      navigate({
                        kind: "album",
                        title: a.title,
                        artist: a.artist,
                      })
                    }
                  >
                    <Cover
                      track={
                        a.tracks.find((t) => t.metadata.artwork) ?? a.tracks[0]
                      }
                      session={session_id}
                    />
                    <strong>{a.title}</strong>
                    <span>{a.artist || "Неизвестный исполнитель"}</span>
                  </button>
                ))}
              </div>
            </section>
          )}
        </>
      )}
      <section>
        <div className="section-heading">
          <h2>Ваши плейлисты</h2>
          <button
            className="text-button"
            onClick={() => navigate({ kind: "playlists" })}
          >
            Открыть
            <ArrowRight size={16} />
          </button>
        </div>
        {playlists.length ? (
          <div className="playlist-home">
            {playlists.slice(0, 4).map((p) => (
              <button
                key={p.playlist_id}
                onClick={() =>
                  navigate({ kind: "playlist", id: p.playlist_id })
                }
              >
                <ListMusic />
                <span>{p.title}</span>
                <ArrowRight size={16} />
              </button>
            ))}
          </div>
        ) : (
          <p className="section-empty">
            Для дороги, вечера или нового открытия — соберите свой первый
            плейлист.
          </p>
        )}
      </section>
    </>
  );
}
export function CollectionScreen({
  route,
  ...props
}: Props & { route: Route }) {
  const { runtime, navigate } = props;
  const collection = useCollection(runtime);
  const { model, run } = runtime;
  const session_id = model.status!.context.session_id;
  let tracks = collection.tracks;
  let title = "Медиатека";
  if (route.kind === "favorites") {
    const liked = new Set(model.projection?.state.favorites.liked ?? []);
    tracks = tracks.filter((t) => liked.has(t.track_id));
    title = "Любимые треки";
  }
  if (route.kind === "album") {
    tracks =
      collection.albums.find(
        (a) => a.title === route.title && a.artist === route.artist,
      )?.tracks ?? [];
    title = route.title;
  }
  if (route.kind === "artist") {
    tracks =
      collection.artists.find((a) => a.name === route.name)?.tracks ?? [];
    title = route.name;
  }
  if (route.kind === "albums" || route.kind === "artists")
    return (
      <>
        <PageHeading
          title={route.kind === "albums" ? "Альбомы" : "Исполнители"}
          subtitle="Из вашей личной коллекции"
        />
        <div className="album-grid">
          {route.kind === "albums"
            ? collection.albums.map((a) => (
                <button
                  className="album-card"
                  key={JSON.stringify([a.title, a.artist])}
                  onClick={() =>
                    navigate({
                      kind: "album",
                      title: a.title,
                      artist: a.artist,
                    })
                  }
                >
                  <Cover
                    session={session_id}
                    track={
                      a.tracks.find((t) => t.metadata.artwork) ?? a.tracks[0]
                    }
                  />
                  <strong>{a.title}</strong>
                  <span>
                    {a.artist || "Неизвестный исполнитель"} · {a.tracks.length}{" "}
                    треков
                  </span>
                </button>
              ))
            : collection.artists.map((a) => (
                <button
                  className="album-card artist-card"
                  key={a.name}
                  onClick={() => navigate({ kind: "artist", name: a.name })}
                >
                  <Cover
                    session={session_id}
                    track={
                      a.tracks.find((t) => t.metadata.artwork) ?? a.tracks[0]
                    }
                  />
                  <strong>{a.name}</strong>
                  <span>{a.tracks.length} треков</span>
                </button>
              ))}
        </div>
        {(route.kind === "albums"
          ? !collection.albums.length
          : !collection.artists.length) && (
          <Empty title="Коллекция ещё не собрана">
            Альбомы и исполнители появятся из метаданных вашей музыки.
          </Empty>
        )}
      </>
    );
  return (
    <>
      <PageHeading
        title={title}
        subtitle={`${tracks.length} композиций · ${duration(tracks.reduce((n, t) => n + t.metadata.duration_ms, 0))}`}
      >
        <button
          className="primary"
          disabled={!tracks.length}
          onClick={() =>
            void run(
              "Воспроизведение",
              () =>
                call("set_queue", {
                  session_id,
                  track_ids: tracks.map((t) => t.track_id),
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
        <button
          onClick={() => navigate({ kind: "imports" })}
          disabled={model.status?.mode !== "ONLINE_READY"}
        >
          <Plus />
          Добавить
        </button>
      </PageHeading>
      {(route.kind === "album" || route.kind === "artist") && (
        <div className="collection-identity">
          <Cover
            session={session_id}
            track={tracks.find((t) => t.metadata.artwork) ?? tracks[0]}
          />
          <div>
            <p>
              {route.kind === "album"
                ? route.artist || "Исполнитель не указан"
                : "Исполнитель"}
            </p>
            <p>
              {route.kind === "album" && tracks[0]?.metadata.year
                ? `${tracks[0].metadata.year} · `
                : ""}
              {tracks.length} композиций
            </p>
          </div>
        </div>
      )}
      <div className="library-tabs">
        {(
          [
            ["library", "Треки"],
            ["albums", "Альбомы"],
            ["artists", "Исполнители"],
            ["playlists", "Плейлисты"],
            ["favorites", "Любимые"],
            ["downloads", "Загрузки"],
          ] as const
        ).map(([kind, label]) => (
          <button
            key={kind}
            aria-current={route.kind === kind ? "page" : undefined}
            onClick={() => navigate({ kind })}
          >
            {label}
          </button>
        ))}
      </div>
      <TrackList {...props} tracks={tracks} />
    </>
  );
}
export function SearchScreen(props: Props) {
  const { runtime, navigate } = props;
  const [query, setQuery] = useState("");
  const [tracks, setTracks] = useState<Track[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");
  const { albums, artists } = useCollection(runtime);
  const session_id = runtime.model.status!.context.session_id;
  useEffect(() => {
    let current = true;
    setLoading(!!query.trim());
    setError("");
    const timer = setTimeout(() => {
      if (!query.trim()) {
        setTracks([]);
        return;
      }
      void call("library", { session_id, query, offset: 0, limit: 500 })
        .then((result) => {
          if (current) setTracks(result);
        })
        .catch((e) => {
          if (current) setError(errorOf(e).message);
        })
        .finally(() => {
          if (current) setLoading(false);
        });
    }, 220);
    return () => {
      current = false;
      clearTimeout(timer);
    };
  }, [query, session_id, runtime.model.projection]);
  const matches = (s: string) =>
    query.trim() &&
    s.toLocaleLowerCase().includes(query.toLocaleLowerCase().trim());
  const foundAlbums = albums
    .filter((a) => matches(`${a.title} ${a.artist}`))
    .slice(0, 8);
  const foundArtists = artists.filter((a) => matches(a.name)).slice(0, 8);
  const foundPlaylists = Object.values(
    runtime.model.projection?.state.playlists ?? {},
  )
    .filter((p) => matches(p.title))
    .slice(0, 8);
  return (
    <>
      <PageHeading
        title="Поиск"
        subtitle="Треки, альбомы, исполнители и ваши плейлисты"
      />
      <label className="search-field global-search">
        <Search />
        <input
          autoFocus
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          maxLength={256}
          placeholder="Что будем слушать?"
          aria-label="Поиск по медиатеке"
        />
      </label>
      {error && <p role="alert">{error}</p>}
      {loading ? (
        <Loading label="Ищем в вашей коллекции…" />
      ) : !query.trim() ? (
        <Empty title="Найдите свою музыку">
          Поиск работает по локальной библиотеке, в том числе без интернета.
        </Empty>
      ) : (
        <>
          <section>
            <div className="section-heading">
              <h2>Композиции</h2>
              <span>
                {tracks.length === 500
                  ? "Первые 500 совпадений"
                  : `${tracks.length} найдено`}
              </span>
            </div>
            <TrackList {...props} tracks={tracks} />
          </section>
          {!!foundAlbums.length && (
            <section>
              <h2>Альбомы</h2>
              <div className="album-grid">
                {foundAlbums.map((a) => (
                  <button
                    className="album-card"
                    key={JSON.stringify([a.title, a.artist])}
                    onClick={() =>
                      navigate({
                        kind: "album",
                        title: a.title,
                        artist: a.artist,
                      })
                    }
                  >
                    <Cover session={session_id} track={a.tracks[0]} />
                    <strong>{a.title}</strong>
                    <span>{a.artist}</span>
                  </button>
                ))}
              </div>
            </section>
          )}
          {!!foundArtists.length && (
            <section>
              <h2>Исполнители</h2>
              <div className="result-links">
                {foundArtists.map((a) => (
                  <button
                    key={a.name}
                    onClick={() => navigate({ kind: "artist", name: a.name })}
                  >
                    {a.name}
                    <ArrowRight />
                  </button>
                ))}
              </div>
            </section>
          )}
          {!!foundPlaylists.length && (
            <section>
              <h2>Плейлисты</h2>
              <div className="result-links">
                {foundPlaylists.map((p) => (
                  <button
                    key={p.playlist_id}
                    onClick={() =>
                      navigate({ kind: "playlist", id: p.playlist_id })
                    }
                  >
                    {p.title}
                    <ArrowRight />
                  </button>
                ))}
              </div>
            </section>
          )}
        </>
      )}
    </>
  );
}
