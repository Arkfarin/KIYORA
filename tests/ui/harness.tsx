import { useKeyboardShortcuts } from "../../src/hooks/useKeyboardShortcuts";
// Isolated component test entry. Vite's production entry never imports this file.
import React, { useState } from "react";
import { createRoot } from "react-dom/client";
import { emit } from "@tauri-apps/api/event";
import { useBackend } from "../../src/useBackend";
import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import { Shell } from "../../src/components/layout/Shell";
import { TrackList } from "../../src/components/music/TrackList";
import { Player } from "../../src/features/playback/Player";
import { PlaylistForm } from "../../src/features/playlists/PlaylistForm";
import { MetadataEditor } from "../../src/features/metadata-editor/MetadataEditor";
import { useNavigation } from "../../src/hooks/useNavigation";
import { usePreferences } from "../../src/hooks/usePreferences";
import { createPlaybackStore } from "../../src/services/playbackState";
import {
  HomeScreen,
  CollectionScreen,
  SearchScreen,
} from "../../src/features/library/LibraryScreens";
import { SettingsScreen } from "../../src/features/settings/Settings";
import { ImportsScreen } from "../../src/features/import/Imports";
import { TrackActions } from "../../src/components/music/TrackActions";
import { PageHeading } from "../../src/components/ui/Primitives";
import { resumePlayback } from "../../src/services/playback";
import { readDraft, writeDraft } from "../../src/services/drafts";
import type { Track } from "../../src/backend";
import type { Model, Runtime } from "../../src/useBackend";

import "@fontsource-variable/manrope";
import "@fontsource-variable/space-grotesk";
import "../../src/styles/app.css";
import "../../src/styles/playback.css";

mockWindows("main");
const params = new URLSearchParams(location.search);
const count = Math.min(10001, Math.max(0, Number(params.get("count") ?? 3)));
const id = (n: number) =>
  `00000000-0000-4000-8000-${n.toString().padStart(12, "0")}`;
const tracks: Track[] = Array.from({ length: count }, (_, n) => ({
  track_id: id(n + 1),
  updated_order: String(n + 10),
  preferred_source_id: null,
  sources: {},
  metadata: {
    title:
      n === 0
        ? "Очень длинное название композиции для проверки узкого экрана и переноса текста"
        : `Тестовая композиция ${n + 1}`,
    performer: "Исполнитель с длинным именем",
    album: "Тестовый альбом",
    album_artist: "",
    genre: "",
    year: 0,
    track_number: 0,
    disc_number: 0,
    bitrate: 0,
    codec: "FLAC",
    duration_ms: 210000,
    artwork: null,
  },
}));
const context = {
  profile_id: id(90001),
  session_id: id(90002),
  vault_id: id(90003),
};
const calls: { command: string; args: any }[] = [];
let notify = () => {};
let failNext = "";
let delayNext = 0;
let sequence = 1;
let backendRenders = 0;
const interactive = params.has("interactive") || params.has("backend");
mockIPC(
  async (command, payload) => {
    if (command === "backend_bootstrap") return model.status;
    if (command === "backend_session_info")
      return {
        context,
        event_sequence: String(sequence),
        diagnostics: [],
        repair_required: false,
        creation: null,
        playback_volume: 1,
        service_running: true,
        api_configured: true,
        telegram_connection: "ready",
      };
    if (command.startsWith("plugin:event|")) return 1;
    if (command === "plugin:dialog|open") return ["/test/music.flac"];
    if (command !== "backend_invoke")
      throw new Error(`Unexpected native call: ${command}`);
    const request = (payload as any).request.request;
    calls.push(request);
    const ok = (result: unknown) => ({
      version: 1,
      result: { Ok: { kind: request.command, result } },
    });
    if (interactive) {
      const a = request.args;
      if (request.command === "list_profiles") return ok(model.profiles);
      if (request.command === "runtime_status") return ok(model.status);
      if (request.command === "auth_status") return ok({ state: "ready" });
      if (request.command === "playback_state") return ok(model.playback);
      if (request.command === "state_snapshot") return ok(model.projection);
      if (["list_jobs", "downloads", "cached_media"].includes(request.command))
        return ok([]);
      if (request.command === "query" && a.query.kind === "imports")
        return ok({ kind: "imports", result: [] });
      if (failNext === request.command) {
        failNext = "";
        return {
          version: 1,
          result: {
            Err: {
              code: "CONFLICT",
              message: "Очередь уже изменилась. Повторите действие.",
              retryable: true,
            },
          },
        };
      }
      if (delayNext) {
        const delay = delayNext;
        delayNext = 0;
        await new Promise((resolve) => setTimeout(resolve, delay));
      }
      if (request.command === "playback_queue")
        return ok({
          info: model.queue,
          offset: a.offset,
          track_ids: queueIds.slice(a.offset, a.offset + a.limit),
        });
      if (request.command === "lyrics")
        return ok({
          version: 1,
          lines: Array.from({ length: 40 }, (_, i) => ({
            text: `Строка песни ${i + 1}`,
            start_ms: params.has("plain") ? null : i * 5000,
            end_ms: null,
          })),
        });
      if (request.command === "library")
        return ok(
          tracks
            .filter((t) =>
              `${t.metadata.title} ${t.metadata.performer}`
                .toLowerCase()
                .includes(a.query.toLowerCase()),
            )
            .slice(a.offset, a.offset + a.limit),
        );
      if (request.command === "listening_history") return ok([]);
      if (request.command === "storage_usage")
        return ok({
          verified_cache: {
            quota_bytes: 10737418240,
            used_bytes: 0,
            entries: 0,
          },
          partial_bytes: 0,
        });
      if (request.command === "control_playback") {
        if (a.action.kind === "seek")
          model.playback = { ...model.playback, position_ms: a.action.value };
        if (a.action.kind === "pause" || a.action.kind === "resume")
          model.playback = {
            ...model.playback,
            state: a.action.kind === "pause" ? "paused" : "playing",
          };
        if (a.action.kind === "next" || a.action.kind === "previous") {
          model.queue = {
            ...model.queue!,
            position: Math.max(
              0,
              Math.min(
                queueIds.length - 1,
                model.queue!.position! + (a.action.kind === "next" ? 1 : -1),
              ),
            ),
            revision: model.queue!.revision + 1,
          };
          model.queue.current_track_id = queueIds[model.queue.position!];
          model.playback = {
            ...model.playback,
            track_id: model.queue.current_track_id,
            position_ms: 0,
          };
        }
        store.publish(model.playback);
        notify();
        return ok(model.playback);
      }
      if (request.command === "set_queue") {
        queueIds = a.track_ids;
        model.queue = {
          ...model.queue!,
          revision: model.queue!.revision + 1,
          position: a.position,
          total: queueIds.length,
          current_track_id: queueIds[a.position],
        };
        model.playback = {
          state: a.autoplay ? "playing" : "paused",
          position_ms: 0,
          duration_ms: 210000,
          track_id: queueIds[a.position],
        };
        store.publish(model.playback);
        notify();
        return ok(model.queue);
      }
      if (request.command === "edit_queue") {
        const q = { ...model.queue! };
        if (a.revision !== q.revision) throw new Error("stale queue revision");
        const action = a.action;
        if (action.kind === "repeat") q.repeat = action.mode;
        if (action.kind === "shuffle") q.shuffle = action.enabled;
        if (action.kind === "move") {
          const [id] = queueIds.splice(action.from, 1);
          queueIds.splice(action.to, 0, id);
        }
        if (action.kind === "remove") queueIds.splice(action.index, 1);
        if (action.kind === "append")
          queueIds.splice(
            action.next ? (q.position ?? -1) + 1 : queueIds.length,
            0,
            ...action.track_ids,
          );
        if (action.kind === "jump") {
          q.position = action.index;
          model.playback = {
            ...model.playback,
            track_id: queueIds[action.index],
            position_ms: 0,
          };
          store.publish(model.playback);
        }
        q.total = queueIds.length;
        q.current_track_id = queueIds[q.position ?? 0];
        q.revision++;
        model.queue = q;
        notify();
        return ok(q);
      }
      if (
        request.command === "vault" &&
        a.request.kind === "list_import_previews"
      )
        return ok({ kind: "import_previews", result: [] });
      if (request.command === "vault" && a.request.kind === "preview_import")
        return ok({
          kind: "import_preview",
          result: {
            preview_id: a.request.payload.preview_id,
            filename: "music.flac",
            metadata: tracks[0].metadata,
            sha256: "a".repeat(64),
            size_bytes: 2048,
            artwork: null,
            lyrics: null,
            warnings: [],
          },
        });
    }
    // Tests inspect requests; no test operation represents successful publication.
    if (["control_playback", "edit_queue", "vault"].includes(request.command))
      return {
        version: 1,
        result: {
          Err: {
            code: params.has("preflight") ? "EDIT_NOT_SENT" : "NETWORK",
            message: "Тестовый разрыв соединения",
            retryable: true,
          },
        },
      };
    throw new Error(`Unexpected component IPC: ${request.command}`);
  },
  { shouldMockEvents: true },
);
const model: Model = {
  loading: false,
  profiles: [
    {
      profile_id: context.profile_id,
      label: "Тестовый профиль",
      owner_id: "42",
    },
  ],
  status: {
    context,
    mode: params.has("offline") ? "RECONNECTING" : "ONLINE_READY",
    vault: null,
  },
  info: null,
  auth: null,
  projection: {
    context,
    event_sequence: "1",
    cursor: "42",
    state: {
      tracks: Object.fromEntries(
        tracks.map((track) => [track.track_id, track]),
      ),
      playlists: {
        [id(90005)]: {
          playlist_id: id(90005),
          title: "Мой плейлист",
          created_order: "8",
          updated_order: "9",
        },
      },
      entries: {},
      favorites: {},
    },
  },
  playback: {
    state: "paused",
    track_id: tracks[0]?.track_id,
    position_ms: 40000,
    duration_ms: 210000,
  },
  queue: {
    queue_id: id(90004),
    revision: 3,
    repeat: "off",
    shuffle: false,
    total: count,
    position: 0,
    current_track_id: tracks[0]?.track_id ?? null,
  },
  imports: [],
  jobs: [],
  downloads: {},
  cached: {},
  progress: null,
  error: null,
  revision: 0,
};
let queueIds = tracks.map((t) => t.track_id);
if (interactive && tracks[0])
  tracks[0].metadata.lyrics = {
    sha256: "a".repeat(64),
    source: {
      source_id: id(80000),
      chat_id: "1",
      message_id: "1",
      sha256: null,
      size_bytes: null,
    },
  };
const store = createPlaybackStore(model.playback);
const runtime: Runtime = {
  model,
  playbackStore: store,
  busy: null,
  run: async (_, operation) => {
    try {
      return await operation();
    } catch {
      return undefined;
    }
  },
  refresh: async () => {},
  clearError: () => {},
};
Object.assign(window, {
  testRuntime: {
    calls,
    model,
    readDraft,
    writeDraft,
    resumePlayback,
    backendRenders: () => backendRenders,
    backendEvent: async (position_ms: number, foreign = false) => {
      if (!foreign) model.playback = { ...model.playback, position_ms };
      await emit("kiyora://backend", {
        sequence: String(++sequence),
        context: foreign ? { ...context, session_id: id(999) } : context,
        name: "playback.state",
        payload: { ...model.playback, position_ms },
      });
    },
    tick: (position_ms: number) => {
      model.playback = { ...model.playback, position_ms };
      store.publish(model.playback);
    },
    fail: (command: string) => {
      failNext = command;
    },
    delay: (ms: number) => {
      delayNext = ms;
    },
  },
});
function Harness() {
  const [, render] = useState(0);
  notify = () => render((n) => n + 1);
  const preferences = usePreferences();
  const { route, navigate, back, canGoBack } = useNavigation(
    model.status!.context.session_id,
  );
  useKeyboardShortcuts(runtime, navigate);
  const [menu, setMenu] = useState<Track | null>(null);
  const [editing, setEditing] = useState(false);
  const [playlistEditing, setPlaylistEditing] = useState(
    params.has("playlist-editor"),
  );
  const [playlistCreating, setPlaylistCreating] = useState(false);
  return (
    <Shell
      runtime={runtime}
      route={route}
      back={canGoBack ? back : undefined}
      navigate={navigate}
      createPlaylist={() => setPlaylistCreating(true)}
      player={
        <Player
          runtime={runtime}
          onMenu={interactive ? setMenu : () => setEditing(true)}
          onEdit={() => setEditing(true)}
        />
      }
    >
      {!interactive && (
        <>
          <PageHeading
            title="Медиатека"
            subtitle={`${count.toLocaleString("ru")} композиций · изолированная проверка компонентов`}
          >
            <button onClick={() => setEditing(true)}>
              Редактировать тестовый трек
            </button>
            <button onClick={() => setPlaylistEditing(true)}>
              Редактировать тестовый плейлист
            </button>
            <button onClick={() => setPlaylistCreating(true)}>
              Создать тестовый плейлист
            </button>
          </PageHeading>
          <TrackList
            runtime={runtime}
            tracks={tracks}
            navigate={navigate}
            onMenu={() => setEditing(true)}
            onDelete={() => {}}
          />
        </>
      )}
      {interactive && (
        <>
          {route.kind === "home" && (
            <HomeScreen
              runtime={runtime}
              navigate={navigate}
              onMenu={setMenu}
              onDelete={() => {}}
            />
          )}
          {[
            "library",
            "favorites",
            "albums",
            "artists",
            "album",
            "artist",
          ].includes(route.kind) && (
            <CollectionScreen
              route={route}
              runtime={runtime}
              navigate={navigate}
              onMenu={setMenu}
              onDelete={() => {}}
            />
          )}
          <div hidden={route.kind !== "search"}>
            <SearchScreen
              active={route.kind === "search"}
              runtime={runtime}
              navigate={navigate}
              onMenu={setMenu}
              onDelete={() => {}}
            />
          </div>
          {route.kind === "settings" && (
            <SettingsScreen runtime={runtime} preferences={preferences} />
          )}
          {route.kind === "imports" && <ImportsScreen runtime={runtime} />}
          {menu && (
            <TrackActions
              runtime={runtime}
              track={menu}
              navigate={navigate}
              onEdit={() => setEditing(true)}
              onDelete={() => {}}
              onClose={() => setMenu(null)}
            />
          )}
        </>
      )}
      {(playlistEditing || playlistCreating) && (
        <PlaylistForm
          runtime={runtime}
          playlist={
            playlistCreating
              ? undefined
              : model.projection!.state.playlists[id(90005)]
          }
          onClose={() => {
            setPlaylistEditing(false);
            setPlaylistCreating(false);
          }}
          onSaved={() => {
            setPlaylistEditing(false);
            setPlaylistCreating(false);
          }}
        />
      )}
      {editing && (
        <MetadataEditor
          runtime={runtime}
          track={tracks[0]}
          onClose={() => setEditing(false)}
        />
      )}
    </Shell>
  );
}
function BackendProbe() {
  const actual = useBackend();
  backendRenders++;
  if (!actual.model.status || !actual.model.projection)
    return <p>Загрузка backend…</p>;
  return (
    <Shell
      runtime={actual}
      route={{ kind: "library" }}
      navigate={() => {}}
      createPlaylist={() => {}}
      player={<Player runtime={actual} onMenu={() => {}} onEdit={() => {}} />}
    >
      <PageHeading title="Медиатека" />
      <TrackList
        runtime={actual}
        tracks={Object.values(actual.model.projection.state.tracks)}
        navigate={() => {}}
        onMenu={() => {}}
        onDelete={() => {}}
      />
    </Shell>
  );
}
createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    {params.has("backend") ? <BackendProbe /> : <Harness />}
  </React.StrictMode>,
);
