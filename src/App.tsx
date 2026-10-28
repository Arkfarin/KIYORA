import { useEffect, useRef, useState } from "react";
import { AlertCircle, X } from "lucide-react";
import { call, errorHelp, mutate, uuid } from "./backend";
import type { Track } from "./backend";
import { useBackend } from "./useBackend";
import { usePreferences } from "./hooks/usePreferences";
import type { Route } from "./app/navigation";
import { Shell } from "./components/layout/Shell";
import { Confirm, IconButton } from "./components/ui/Primitives";
import { TrackActions } from "./components/music/TrackActions";
import { Onboarding } from "./features/onboarding/Onboarding";
import {
  HomeScreen,
  CollectionScreen,
  SearchScreen,
} from "./features/library/LibraryScreens";
import {
  PlaylistForm,
  PlaylistScreen,
  PlaylistsScreen,
} from "./features/playlists/Playlists";
import { Player } from "./features/playback/Player";
import { ImportsScreen } from "./features/import/Imports";
import { MetadataEditor } from "./features/metadata-editor/MetadataEditor";
import { DownloadsScreen } from "./features/downloads/Downloads";
import { SettingsScreen } from "./features/settings/Settings";
import "@fontsource-variable/manrope";
import "@fontsource-variable/space-grotesk";
import "./styles/app.css";
export default function App() {
  const runtime = useBackend();
  const preferences = usePreferences();
  const { model, run } = runtime;
  const [route, setRoute] = useState<Route>({ kind: "home" });
  const [allowLocal, setAllowLocal] = useState(false);
  const [menu, setMenu] = useState<Track | null>(null);
  const [editor, setEditor] = useState<Track | null>(null);
  const [createPlaylist, setCreatePlaylist] = useState(false);
  const [deleting, setDeleting] = useState<{ track: Track; op: string }[]>([]);
  const [removing, setRemoving] = useState(false);
  const session = model.status?.context.session_id;
  const [openedSession, setOpenedSession] = useState<string>();
  const navigate = (route: Route) => {
    setRoute(route);
    document.getElementById("main-content")?.scrollTo({ top: 0 });
  };
  const current = useRef({ runtime, navigate });
  current.current = { runtime, navigate };
  useEffect(() => {
    setMenu(null);
    setEditor(null);
    setDeleting([]);
    setCreatePlaylist(false);
    setAllowLocal(false);
    setRoute({ kind: "home" });
  }, [session]);
  const usable =
    !!model.status?.context.vault_id &&
    !!model.projection &&
    model.projection.context.vault_id === model.status.context.vault_id &&
    !model.info?.repair_required &&
    (model.status.mode === "ONLINE_READY" ||
      allowLocal ||
      openedSession === session ||
      (model.projection.cursor !== "0" &&
        !model.info?.creation &&
        model.status.mode !== "AUTH_REQUIRED"));
  useEffect(() => {
    if (usable) setOpenedSession(session);
  }, [usable, session]);
  useEffect(() => {
    const key = (event: KeyboardEvent) => {
      if (
        event.repeat ||
        event.altKey ||
        document.querySelector("dialog[open]")
      )
        return;
      const target = event.target as HTMLElement;
      if (target.closest("input,textarea,select,[contenteditable=true]"))
        return;
      const { runtime, navigate } = current.current;
      const session_id = runtime.model.status?.context.session_id;
      if (!session_id) return;
      if (event.ctrlKey || event.metaKey) {
        if (event.key.toLowerCase() === "k") {
          event.preventDefault();
          navigate({ kind: "search" });
        } else if (event.key.toLowerCase() === "l") {
          event.preventDefault();
          navigate({ kind: "library" });
        } else if (event.key === "ArrowRight" || event.key === "ArrowLeft") {
          event.preventDefault();
          void runtime.run(
            "Сменить композицию",
            () =>
              call("control_playback", {
                session_id,
                action: {
                  kind: event.key === "ArrowRight" ? "next" : "previous",
                },
              }),
            false,
          );
        }
      } else if (event.code === "Space" && !target.closest("button,a")) {
        event.preventDefault();
        const playback = runtime.model.playback;
        const queue = runtime.model.queue;
        if (["playing", "paused", "buffering"].includes(playback.state))
          void runtime.run(
            "Пауза / воспроизведение",
            () =>
              call("control_playback", {
                session_id,
                action: {
                  kind: playback.state === "playing" ? "pause" : "resume",
                },
              }),
            false,
          );
        else if (queue?.current_track_id)
          void runtime.run(
            "Воспроизведение",
            () =>
              call("edit_queue", {
                session_id,
                queue_id: queue.queue_id,
                revision: queue.revision,
                action: { kind: "jump", index: queue.position ?? 0 },
              }),
            false,
          );
      }
    };
    window.addEventListener("keydown", key);
    return () => window.removeEventListener("keydown", key);
  }, []);
  const requestDelete = (tracks: Track[]) =>
    setDeleting(tracks.map((track) => ({ track, op: uuid() })));
  const common = {
    runtime,
    navigate,
    onMenu: setMenu,
    onDelete: requestDelete,
  };
  if (!usable)
    return (
      <Onboarding runtime={runtime} openLocal={() => setAllowLocal(true)} />
    );
  return (
    <>
      <Shell
        runtime={runtime}
        route={route}
        navigate={navigate}
        createPlaylist={() => setCreatePlaylist(true)}
        player={
          <Player runtime={runtime} onMenu={setMenu} onEdit={setEditor} />
        }
      >
        {model.error && (
          <div className="error-banner" role="alert">
            <AlertCircle />
            <div>
              <strong>{model.error.message}</strong>
              <p>
                {errorHelp[model.error.code] ??
                  "Обновите состояние и повторите действие после проверки соединения."}
              </p>
            </div>
            <IconButton
              label="Закрыть сообщение об ошибке"
              onClick={runtime.clearError}
            >
              <X />
            </IconButton>
          </div>
        )}
        {runtime.busy && (
          <div className="operation-status" role="status">
            {runtime.busy}…
          </div>
        )}
        {route.kind === "home" && <HomeScreen {...common} />}
        {[
          "library",
          "favorites",
          "albums",
          "artists",
          "album",
          "artist",
        ].includes(route.kind) && (
          <CollectionScreen {...common} route={route} />
        )}
        {route.kind === "search" && <SearchScreen {...common} />}
        {route.kind === "playlists" && (
          <PlaylistsScreen
            runtime={runtime}
            navigate={navigate}
            create={() => setCreatePlaylist(true)}
          />
        )}
        {route.kind === "playlist" && (
          <PlaylistScreen
            runtime={runtime}
            id={route.id}
            navigate={navigate}
            onMenu={setMenu}
          />
        )}
        {route.kind === "imports" && <ImportsScreen runtime={runtime} />}
        {route.kind === "downloads" && <DownloadsScreen runtime={runtime} />}
        {route.kind === "settings" && (
          <SettingsScreen runtime={runtime} preferences={preferences} />
        )}
      </Shell>
      {menu && (
        <TrackActions
          runtime={runtime}
          track={menu}
          navigate={navigate}
          onEdit={setEditor}
          onDelete={requestDelete}
          onClose={() => setMenu(null)}
        />
      )}
      {editor && (
        <MetadataEditor
          key={editor.track_id}
          runtime={runtime}
          track={editor}
          onClose={() => setEditor(null)}
        />
      )}
      {createPlaylist && (
        <PlaylistForm
          runtime={runtime}
          onClose={() => setCreatePlaylist(false)}
          onSaved={(id) => {
            setCreatePlaylist(false);
            navigate({ kind: "playlist", id });
          }}
        />
      )}
      {!!deleting.length && (
        <Confirm
          title={
            deleting.length === 1
              ? "Удалить из библиотеки?"
              : `Удалить ${deleting.length} композиций?`
          }
          confirmLabel="Удалить из библиотеки"
          busy={removing}
          onClose={() => {
            if (!removing) setDeleting([]);
          }}
          onConfirm={() => {
            if (removing) return;
            setRemoving(true);
            void run("Удаление из библиотеки", async () => {
              for (const item of deleting) {
                await mutate(
                  session!,
                  item.track.track_id,
                  { action: "track.delete", payload: {} },
                  item.op,
                );
                setDeleting((items) => items.filter((i) => i.op !== item.op));
              }
            }).finally(() => setRemoving(false));
          }}
        >
          Композиции исчезнут из общей библиотеки и плейлистов на всех
          устройствах. Аудиофайлы в Telegram автоматически не удаляются.
        </Confirm>
      )}
    </>
  );
}
