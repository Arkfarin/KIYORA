import { invoke } from "@tauri-apps/api/core";

export interface ApiError {
  code: string;
  message: string;
  retryable: boolean;
  retry_after_seconds?: number;
}
export interface Context {
  profile_id: string;
  session_id: string;
  vault_id: string | null;
}
export interface Profile {
  profile_id: string;
  label: string;
  owner_id: string | null;
}
export type Mode =
  | "AUTH_REQUIRED"
  | "DISCOVERING"
  | "SYNCING"
  | "ONLINE_READY"
  | "RECONNECTING"
  | "DEGRADED_READONLY";
export interface Status {
  context: Context;
  mode: Mode;
  vault: null | {
    mode: Mode;
    vault_id: string;
    generation: number;
    cursor: string;
    verified_commit_id: string | null;
  };
}
export interface Auth {
  state: string;
  link?: string;
  expected_length?: number;
  code_info?: {
    delivery: string;
    expected_length: number | null;
    timeout_seconds: number | null;
    next_delivery: string | null;
  };
}
export interface Diagnostic {
  sequence: string;
  context: Context;
  detail: string;
  error: ApiError | null;
  progress: Progress | null;
}
export interface SessionInfo {
  context: Context;
  event_sequence: string;
  chat_id: string | null;
  api_configured: boolean;
  telegram_connection: string;
  service_running: boolean;
  repair_required: boolean;
  playback_volume: number;
  creation: null | { vault_id: string; chat_id: string | null; stage: string };
  diagnostics: Diagnostic[];
}
export interface Source {
  source_id: string;
  chat_id: string;
  message_id: string;
  sha256: string | null;
  size_bytes: number | null;
}
export interface Metadata {
  album_artist: string;
  year: number;
  genre: string;
  track_number: number;
  disc_number: number;
  bitrate: number;
  codec: string;
  composer?: string;
  comment?: string;
  copyright?: string;
  lyrics?: { sha256: string; source: Source } | null;
  title: string;
  performer: string;
  album: string;
  duration_ms: number;
  artwork: null | { sha256: string; mime_type: string; source: Source | null };
}
export interface Track {
  preferred_source_id?: string | null;
  updated_order: string;
  track_id: string;
  metadata: Metadata;
  sources: Record<string, Source>;
}
export interface Playlist {
  artwork?: Metadata["artwork"];
  created_order: string;
  updated_order: string;
  playlist_id: string;
  title: string;
}
export interface PlaylistDetail {
  playlist: Playlist;
  total: number;
  offset: number;
  items: {
    position: number;
    move_up_after: string | null;
    next_entry_id: string | null;
    entry: {
      entry_id: string;
      track_id: string;
      after_entry_id: string | null;
    };
    track: Track;
  }[];
}
export interface Projection {
  context: Context;
  event_sequence: string;
  cursor: string;
  state: {
    tracks: Record<string, Track>;
    playlists: Record<string, Playlist>;
    favorites: Record<string, string[]>;
    entries: Record<
      string,
      {
        entry_id: string;
        track_id: string;
        playlist_id: string;
        after_entry_id: string | null;
      }
    >;
  };
}
export interface Playback {
  state: "idle" | "ended" | "buffering" | "playing" | "paused" | "failed";
  track_id?: string;
  position_ms?: number;
  duration_ms?: number;
  code?: string;
}
export interface Queue {
  revision: number;
  repeat: "off" | "queue" | "track";
  shuffle: boolean;
  queue_id: string;
  total: number;
  position: number | null;
  current_track_id: string | null;
}
export interface Progress {
  operation: string;
  phase: string;
  completed: number;
  total: number | null;
}
export interface ImportInfo {
  op_id: string;
  track_id: string;
  source_id: string;
  metadata: Metadata;
  stage: "staging" | "prepared" | "upload_unknown" | "uploaded" | "complete";
  size_bytes: number;
}
export interface Download {
  download_id: string;
  track_id: string;
  state: string;
  bytes_downloaded: number;
  total_bytes: number | null;
  pinned: boolean;
}
export interface Command {
  intent: { op_id: string };
  status: string;
  reason?: string;
}
export interface VaultCandidate {
  protocol: number;
  channel: { chat_id: string; title: string };
  init: { vault_id: string };
  init_message_id: string;
}
export interface Artwork {
  mime_type: string;
  sha256: string;
  bytes: number[];
}
export interface Job {
  job_id: string;
  state: "running" | "succeeded" | "failed" | "cancelled" | "interrupted";
  completed: number;
  total: number;
  error: ApiError | null;
  result: { kind: string; result: VaultReply } | null;
}
export interface LyricLine {
  start_ms: number | null;
  end_ms: number | null;
  text: string;
}
export interface LyricsDocument {
  version: 1;
  language: string | null;
  lines: LyricLine[];
}
export type MetadataPatch = Partial<
  Pick<
    Metadata,
    | "title"
    | "performer"
    | "album"
    | "album_artist"
    | "year"
    | "genre"
    | "track_number"
    | "disc_number"
    | "composer"
    | "comment"
    | "copyright"
  >
> & {
  clear_artwork?: boolean;
  clear_lyrics?: boolean;
  expected_order?: string;
};
export interface MediaEdit {
  patch: MetadataPatch;
  artwork: number[] | null;
  lyrics: LyricsDocument | null;
}
export interface ImportPreview {
  preview_id: string;
  filename: string;
  metadata: Metadata;
  sha256: string;
  size_bytes: number;
  artwork: Artwork | null;
  lyrics: LyricsDocument | null;
  warnings: string[];
}
export type QueueAction =
  | { kind: "append"; track_ids: string[]; next: boolean }
  | { kind: "remove" | "jump"; index: number }
  | { kind: "move"; from: number; to: number }
  | { kind: "repeat"; mode: Queue["repeat"] }
  | { kind: "shuffle"; enabled: boolean };
export interface CacheStats {
  entries: number;
  pinned_entries: number;
  partial_entries: number;
  partial_bytes: number;
  reserved_bytes: number;
  used_bytes: number;
  quota_bytes: number;
}
export interface StorageUsage {
  profile_bytes: number;
  verified_cache: CacheStats;
  tdlib_files_bytes: number;
  tdlib_database_bytes: number;
  projection_bytes: number;
  staged_import_bytes: number;
}
export type MutationAction =
  | { action: "favorite.set"; payload: { set_name: string; value: boolean } }
  | {
      action: "track.delete" | "playlist.delete";
      payload: Record<string, never>;
    }
  | { action: "track.metadata.update"; payload: { patch: MetadataPatch } }
  | { action: "playlist.rename"; payload: { title: string } }
  | {
      action: "playlist.entry.move";
      payload: { entry_id: string; after_entry_id: string | null };
    }
  | { action: "playlist.create"; payload: { title: string } }
  | {
      action: "playlist.track.add";
      payload: {
        track_id: string;
        entry_id: string;
        after_entry_id: string | null;
      };
    }
  | { action: "playlist.track.remove"; payload: { entry_id: string } };
export interface PlaylistEdit {
  title: string;
  artwork: number[] | null;
  clear_artwork: boolean;
  expected_order: string;
}
export type VaultRequest =
  | {
      kind: "edit_playlist";
      payload: { op_id: string; playlist_id: string; edit: PlaylistEdit };
    }
  | {
      kind: "remove_cached_media";
      payload: { track_id: string; source_id: string };
    }
  | { kind: "list_import_previews" }
  | { kind: "preview_import"; payload: { path: string; preview_id: string } }
  | { kind: "discard_import_preview"; payload: { preview_id: string } }
  | {
      kind: "import_prepared";
      payload: {
        preview_id: string;
        track_id: string;
        source_id: string;
        edit: MediaEdit | null;
        write_tags: boolean;
        allow_duplicate: boolean;
      };
    }
  | {
      kind: "edit_metadata" | "rewrite_track";
      payload: { op_id: string; track_id: string; edit: MediaEdit };
    }
  | {
      kind: "share_track";
      payload: {
        op_id: string;
        track_id: string;
        source_id: string;
        recipient_chat_id: string;
      };
    }
  | { kind: "cancel_download"; payload: { download_id: string } }
  | {
      kind: "pin_cached_media";
      payload: { track_id: string; source_id: string; pinned: boolean };
    }
  | { kind: "set_cache_quota"; payload: { bytes: number } }
  | { kind: "clear_local_cache" | "snapshot" | "verify" }
  | { kind: "import_backup"; payload: { path: string } }
  | { kind: "sync" | "initialize" | "diagnose" | "get_cache_stats" }
  | { kind: "resume_import"; payload: { op_id: string } }
  | {
      kind: "import";
      payload: {
        path: string;
        track_id: string;
        source_id: string;
        op_id: string;
        allow_duplicate: boolean;
      };
    }
  | {
      kind: "download_for_offline";
      payload: {
        download_id: string;
        track_id: string;
        source_id: string;
        pinned: boolean;
      };
    }
  | {
      kind: "get_download_status";
      payload: { download_id: string; track_id: string; source_id: string };
    }
  | {
      kind: "mutate";
      payload: { op_id: string; entity_id: string; action: MutationAction };
    };
export interface VaultReply {
  kind: string;
  result?: unknown;
}
export interface BackendEvent {
  sequence: string;
  context: Context;
  name: string;
  payload: unknown;
}
export type PlaybackAction =
  | { kind: "next" | "previous" | "pause" | "resume" | "stop" }
  | { kind: "seek" | "set_volume"; value: number };
type Session = { session_id: string };
interface Commands {
  listening_history: [
    { session_id: string },
    { track_id: string; completed_plays: number; last_played_ms: number }[],
  ];
  downloads: [{ session_id: string }, Download[]];
  cached_media: [
    Session,
    { key: string; size_bytes: number; pinned: boolean; sha256: string }[],
  ];
  library: [
    Session & { query: string | null; offset: number; limit: number },
    Track[],
  ];
  discover_vaults_deep: [Session, VaultCandidate[]];
  lyrics: [Session & { track_id: string }, LyricsDocument | null];
  parse_lyrics: [{ text: string; format: string }, LyricsDocument | null];
  prepare_artwork: [
    { bytes: number[]; crop: { x: number; y: number; size: number } | null },
    Artwork | null,
  ];
  edit_queue: [
    Session & { queue_id: string; revision: number; action: QueueAction },
    Queue,
  ];
  storage_usage: [Session, StorageUsage];
  logout: [Session, undefined];
  disconnect: [Session, undefined];
  export_backup: [
    Session & { path: string; kind: "metadata" | "full" },
    unknown,
  ];
  list_profiles: [undefined, Profile[]];
  create_profile: [{ label: string }, Profile];
  switch_profile: [{ profile_id: string }, Status];
  runtime_status: [undefined, Status];
  auth_status: [Session, Auth];
  connect_telegram: [Session & { api_id: number; api_hash: string }, Auth];
  authenticate: [Session & { input: { kind: string; value?: string } }, Auth];
  discover_vaults: [Session, VaultCandidate[]];
  attach_vault: [Session & { chat_id: string }, Status];
  create_vault: [Session & { title?: string }, Status];
  resume_vault_creation: [Session & { chat_id: string }, Status];
  state_snapshot: [Session, Projection];
  playback_state: [Session, Playback];
  playback_queue: [
    Session & { offset: number; limit: number },
    { info: Queue; track_ids: string[] },
  ];
  set_queue: [
    Session & { track_ids: string[]; position: number; autoplay: boolean },
    Queue,
  ];
  queue_playlist: [
    Session & { playlist_id: string; position: number; autoplay: boolean },
    Queue,
  ];
  control_playback: [Session & { action: PlaybackAction }, Playback];
  artwork: [Session & { track_id: string }, Artwork | null];
  playlist_artwork: [Session & { playlist_id: string }, Artwork | null];
  query: [
    Session & {
      query: { kind: string; payload?: Record<string, string | number> };
    },
    { kind: string; result: unknown },
  ];
  vault: [Session & { request: VaultRequest }, VaultReply];
  start_job: [
    Session & {
      job_id: string;
      request: { kind: "vault"; payload: VaultRequest };
      resume?: boolean;
    },
    Job,
  ];
  list_jobs: [Session, Job[]];
  cancel_job: [Session & { job_id: string }, Job];
  repair_local_database: [Session, unknown];
}
export async function call<K extends keyof Commands>(
  command: K,
  args: Commands[K][0],
): Promise<Commands[K][1]> {
  const response = await invoke<{
    version: number;
    result: { Ok?: { kind: string; result?: Commands[K][1] }; Err?: ApiError };
  }>("backend_invoke", {
    request: {
      version: 1,
      request: args === undefined ? { command } : { command, args },
    },
  });
  if (response.version !== 1)
    throw {
      code: "PROTOCOL_UNSUPPORTED",
      message: "Несовместимая версия backend",
      retryable: false,
    };
  if (response.result.Err) throw response.result.Err;
  if (!response.result.Ok)
    throw {
      code: "INTERNAL",
      message: "Backend returned no response",
      retryable: false,
    };
  return response.result.Ok.result as Commands[K][1];
}
export const bootstrap = () => invoke<Status | null>("backend_bootstrap");
export const sessionInfo = (session_id: string) =>
  invoke<SessionInfo>("backend_session_info", { sessionId: session_id });
export const reconnect = (session_id: string) =>
  invoke<Auth>("backend_reconnect", { sessionId: session_id });
export const resumeJob = (session_id: string, job_id: string) =>
  invoke<Job>("backend_resume_job", { sessionId: session_id, jobId: job_id });
export const authQr = (session_id: string) =>
  invoke<string | null>("backend_auth_qr", { sessionId: session_id });
export function errorOf(value: unknown): ApiError {
  if (
    typeof value === "object" &&
    value !== null &&
    "code" in value &&
    "message" in value
  )
    return value as ApiError;
  // Never display raw IPC arguments, native errors or stringified credentials.
  return {
    code: "INTERNAL",
    message:
      "Не удалось получить ответ Rust backend. Повторите обновление состояния.",
    retryable: true,
  };
}
export const errorHelp: Record<string, string> = {
  INTEGRITY:
    "Сохранённые данные остаются на месте. Посмотрите причину; для неполного Vault продолжите инициализацию в том же канале.",
  CONFLICT:
    "Обновите состояние. Если создание прервалось, продолжите сохранённую операцию в существующем канале.",
  NO_VAULT:
    "Проверьте binding в Settings. Подключите канал или повторите синхронизацию; при отсутствии snapshot продолжите инициализацию.",
  READONLY:
    "Облачные изменения доступны только после ONLINE_READY. Локальная библиотека и кэш остаются доступны.",
  NETWORK: "Дождитесь соединения Telegram и повторите синхронизацию.",
  OFFLINE:
    "Можно читать библиотеку и слушать сохранённый кэш. Повторите синхронизацию после подключения.",
  AUTH_REQUIRED: "Откройте Settings и завершите вход в Telegram.",
  CANCELLED:
    "Операция остановлена. Подтверждённые данные сохранены; проверьте Imports перед повторной отправкой.",
  DATABASE_CORRUPT:
    "В Settings доступно восстановление из проверенного snapshot с сохранением исходной SQLite.",
  NATIVE_UNAVAILABLE:
    "Укажите закреплённую TDLib 1.8.69 через KIYORA_TDJSON и перезапустите приложение.",
  PROFILE_BUSY:
    "Этот профиль занят другим процессом. Закройте использующий его CLI или второй экземпляр Kiyora.",
  SECURE_STORAGE_UNAVAILABLE:
    "Разблокируйте системное хранилище секретов и повторите подключение.",
  TIMEOUT:
    "Исход отправки может быть неизвестен. Возобновите существующую операцию, сохранив её идентификатор.",
};
export function duration(ms = 0) {
  const seconds = Math.floor(ms / 1000);
  return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}`;
}
export const uuid = () => crypto.randomUUID();

export async function mutate(
  session_id: string,
  entity_id: string,
  action: MutationAction,
  op_id: string = uuid(),
) {
  const reply = await call("vault", {
    session_id,
    request: { kind: "mutate", payload: { op_id, entity_id, action } },
  });
  assertApplied(reply);
  return reply;
}
export function assertApplied(reply: VaultReply) {
  if (
    reply.kind !== "command" ||
    (reply.result as Command)?.status !== "APPLIED"
  )
    throw {
      code: "CONFLICT",
      message:
        "Изменение не подтверждено. Обновите состояние операции перед повтором.",
      retryable: false,
    };
}
export const bytesLabel = (n: number) =>
  n < 1024
    ? `${n} Б`
    : n < 1048576
      ? `${(n / 1024).toFixed(1)} КБ`
      : n < 1073741824
        ? `${(n / 1048576).toFixed(1)} МБ`
        : `${(n / 1073741824).toFixed(1)} ГБ`;
