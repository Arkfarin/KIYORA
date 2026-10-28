// Isolated component test entry. Vite's production entry never imports this file.
import React, { useState } from "react";
import { createRoot } from "react-dom/client";
import { mockIPC } from "@tauri-apps/api/mocks";
import { Shell } from "../../src/components/layout/Shell";
import { TrackList } from "../../src/components/music/TrackList";
import { Player } from "../../src/features/playback/Player";
import { MetadataEditor } from "../../src/features/metadata-editor/MetadataEditor";
import { PageHeading } from "../../src/components/ui/Primitives";
import { resumePlayback } from "../../src/services/playback";
import { readDraft, writeDraft } from "../../src/services/drafts";
import type { Track } from "../../src/backend";
import type { Model, Runtime } from "../../src/useBackend";
import type { Route } from "../../src/app/navigation";
import "@fontsource-variable/manrope";
import "@fontsource-variable/space-grotesk";
import "../../src/styles/app.css";

const params = new URLSearchParams(location.search);
const count = Math.min(10001, Math.max(1, Number(params.get("count") ?? 3)));
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
mockIPC((command, payload) => {
  if (command !== "backend_invoke")
    throw new Error(`Unexpected native call: ${command}`);
  const request = (payload as any).request.request;
  calls.push(request);
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
});
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
      playlists: {},
      entries: {},
      favorites: {},
    },
  },
  playback: {
    state: "paused",
    track_id: tracks[0].track_id,
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
    current_track_id: tracks[0].track_id,
  },
  imports: [],
  jobs: [],
  downloads: {},
  cached: {},
  progress: null,
  error: null,
  revision: 0,
};
const runtime: Runtime = {
  model,
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
  testRuntime: { calls, model, readDraft, writeDraft, resumePlayback },
});
function Harness() {
  const [route, navigate] = useState<Route>({ kind: "library" });
  const [editing, setEditing] = useState(false);
  return (
    <Shell
      runtime={runtime}
      route={route}
      navigate={navigate}
      createPlaylist={() => {}}
      player={
        <Player
          runtime={runtime}
          onMenu={() => setEditing(true)}
          onEdit={() => setEditing(true)}
        />
      }
    >
      <PageHeading
        title="Медиатека"
        subtitle={`${count.toLocaleString("ru")} композиций · изолированная проверка компонентов`}
      >
        <button onClick={() => setEditing(true)}>
          Редактировать тестовый трек
        </button>
      </PageHeading>
      <TrackList
        runtime={runtime}
        tracks={tracks}
        navigate={navigate}
        onMenu={() => setEditing(true)}
        onDelete={() => {}}
      />
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
createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <Harness />
  </React.StrictMode>,
);
