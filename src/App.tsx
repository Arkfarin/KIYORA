import { useKeyboardShortcuts } from "./hooks/useKeyboardShortcuts";
import { useEffect, useState } from "react";
import { AlertCircle, X } from "lucide-react";
import { errorHelp, mutate, uuid } from "./backend";
import type { Track } from "./backend";
import { useBackend } from "./useBackend";
import { usePreferences } from "./hooks/usePreferences";
import { useNavigation } from "./hooks/useNavigation";
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
import "./styles/playback.css";
export default function App() {
  const runtime = useBackend();
  const preferences = usePreferences();
  const { model, run } = runtime;
  const { route, navigate, back, canGoBack } = useNavigation(
    model.status?.context.session_id,
  );
  const [allowLocal, setAllowLocal] = useState(false);
  const [menu, setMenu] = useState<Track | null>(null);
  const [editor, setEditor] = useState<Track | null>(null);
  const [createPlaylist, setCreatePlaylist] = useState(false);
  const [deleting, setDeleting] = useState<{ track: Track; op: string }[]>([]);
  const [removing, setRemoving] = useState(false);
  const session = model.status?.context.session_id;
  const [openedSession, setOpenedSession] = useState<string>();
  useKeyboardShortcuts(runtime, navigate);
  useEffect(() => {
    setMenu(null);
    setEditor(null);
    setDeleting([]);
    setCreatePlaylist(false);
    setAllowLocal(false);
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
        back={canGoBack ? back : undefined}
        navigate={navigate}
        createPlaylist={() => setCreatePlaylist(true)}
        player={
          <Player
            key={session}
            runtime={runtime}
            onMenu={setMenu}
            onEdit={setEditor}
          />
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
        <div hidden={route.kind !== "search"}>
          <SearchScreen {...common} active={route.kind === "search"} />
        </div>
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
        {route.kind === "downloads" && (
          <DownloadsScreen runtime={runtime} navigate={navigate} />
        )}
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
