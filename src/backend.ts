import { invoke } from "@tauri-apps/api/core";

export interface ApiError { code: string; message: string; retryable: boolean; retry_after_seconds?: number }
export interface Context { profile_id: string; session_id: string; vault_id: string | null }
export interface Profile { profile_id: string; label: string; owner_id: string | null }
export type Mode = "AUTH_REQUIRED" | "DISCOVERING" | "SYNCING" | "ONLINE_READY" | "RECONNECTING" | "DEGRADED_READONLY";
export interface Status { context: Context; mode: Mode; vault: null | { mode: Mode; vault_id: string; generation: number; cursor: string; verified_commit_id: string | null } }
export interface Auth { state: string; link?: string; expected_length?: number; code_info?: { delivery: string; expected_length: number | null; timeout_seconds: number | null; next_delivery: string | null } }
export interface Diagnostic { sequence: string; context: Context; detail: string; error: ApiError | null; progress: Progress | null }
export interface SessionInfo {
  context: Context; event_sequence: string; chat_id: string | null; api_configured: boolean; telegram_connection: string;
  service_running: boolean; repair_required: boolean; playback_volume: number;
  creation: null | { vault_id: string; chat_id: string | null; stage: string };
  diagnostics: Diagnostic[];
}
export interface Source { source_id: string; chat_id: string; message_id: string; sha256: string | null; size_bytes: number | null }
export interface Metadata { title: string; performer: string; album: string; duration_ms: number; artwork: null | { sha256: string; mime_type: string; source: Source | null } }
export interface Track { track_id: string; metadata: Metadata; sources: Record<string, Source> }
export interface Playlist { playlist_id: string; title: string }
export interface PlaylistDetail { playlist: Playlist; total: number; offset: number; items: { entry: { entry_id: string; track_id: string; after_entry_id: string | null }; track: Track }[] }
export interface Projection { context: Context; event_sequence: string; cursor: string; state: { tracks: Record<string, Track>; playlists: Record<string, Playlist> } }
export interface Playback { state: "idle" | "ended" | "buffering" | "playing" | "paused" | "failed"; track_id?: string; position_ms?: number; duration_ms?: number; code?: string }
export interface Queue { queue_id: string; total: number; position: number | null; current_track_id: string | null }
export interface Progress { operation: string; phase: string; completed: number; total: number | null }
export interface ImportInfo { op_id: string; track_id: string; source_id: string; metadata: Metadata; stage: "staging" | "prepared" | "upload_unknown" | "uploaded" | "complete"; size_bytes: number }
export interface Download { download_id: string; track_id: string; state: string; bytes_downloaded: number; total_bytes: number | null; pinned: boolean }
export interface Command { intent: { op_id: string }; status: string; reason?: string }
export interface VaultCandidate { protocol: number; channel: { chat_id: string; title: string }; init: { vault_id: string }; init_message_id: string }
export interface Artwork { mime_type: string; sha256: string; bytes: number[] }
export interface Job { job_id: string; state: "running" | "succeeded" | "failed" | "cancelled" | "interrupted"; completed: number; total: number; error: ApiError | null; result: { kind: string; result: VaultReply } | null }
export type MutationAction =
  | { action: "playlist.create"; payload: { title: string } }
  | { action: "playlist.track.add"; payload: { track_id: string; entry_id: string; after_entry_id: string | null } }
  | { action: "playlist.track.remove"; payload: { entry_id: string } };
export type VaultRequest =
  | { kind: "sync" | "initialize" | "diagnose" | "get_cache_stats" }
  | { kind: "resume_import"; payload: { op_id: string } }
  | { kind: "import"; payload: { path: string; track_id: string; source_id: string; op_id: string; allow_duplicate: boolean } }
  | { kind: "download_for_offline"; payload: { download_id: string; track_id: string; source_id: string; pinned: boolean } }
  | { kind: "get_download_status"; payload: { download_id: string; track_id: string; source_id: string } }
  | { kind: "mutate"; payload: { op_id: string; entity_id: string; action: MutationAction } };
export interface VaultReply { kind: string; result?: unknown }
export interface BackendEvent { sequence: string; context: Context; name: string; payload: unknown }
export type PlaybackAction = { kind: "next" | "previous" | "pause" | "resume" | "stop" } | { kind: "seek" | "set_volume"; value: number };
type Session = { session_id: string };
interface Commands {
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
  playback_queue: [Session & { offset: number; limit: number }, { info: Queue; track_ids: string[] }];
  set_queue: [Session & { track_ids: string[]; position: number; autoplay: boolean }, Queue];
  queue_playlist: [Session & { playlist_id: string; position: number; autoplay: boolean }, Queue];
  control_playback: [Session & { action: PlaybackAction }, Playback];
  artwork: [Session & { track_id: string }, Artwork | null];
  query: [Session & { query: { kind: string; payload?: Record<string, string | number> } }, { kind: string; result: unknown }];
  vault: [Session & { request: VaultRequest }, VaultReply];
  start_job: [Session & { job_id: string; request: { kind: "vault"; payload: VaultRequest }; resume?: boolean }, Job];
  list_jobs: [Session, Job[]];
  cancel_job: [Session & { job_id: string }, Job];
  repair_local_database: [Session, unknown];
}
export async function call<K extends keyof Commands>(command: K, args: Commands[K][0]): Promise<Commands[K][1]> {
  const response = await invoke<{ version: number; result: { Ok?: { kind: string; result?: Commands[K][1] }; Err?: ApiError } }>("backend_invoke", {
    request: { version: 1, request: args === undefined ? { command } : { command, args } },
  });
  if (response.result.Err) throw response.result.Err;
  if (!response.result.Ok) throw { code: "INTERNAL", message: "Backend returned no response", retryable: false };
  return response.result.Ok.result as Commands[K][1];
}
export const bootstrap = () => invoke<Status | null>("backend_bootstrap");
export const sessionInfo = (session_id: string) => invoke<SessionInfo>("backend_session_info", { sessionId: session_id });
export const reconnect = (session_id: string) => invoke<Auth>("backend_reconnect", { sessionId: session_id });
export const resumeJob = (session_id: string, job_id: string) => invoke<Job>("backend_resume_job", { sessionId: session_id, jobId: job_id });
export const authQr = (session_id: string) => invoke<string | null>("backend_auth_qr", { sessionId: session_id });
export function errorOf(value: unknown): ApiError {
  if (typeof value === "object" && value !== null && "code" in value && "message" in value) return value as ApiError;
  // Never display raw IPC arguments, native errors or stringified credentials.
  return { code: "INTERNAL", message: "Не удалось получить ответ Rust backend. Повторите обновление состояния.", retryable: true };
}
export const errorHelp: Record<string, string> = {
  INTEGRITY: "Сохранённые данные остаются на месте. Посмотрите причину; для неполного Vault продолжите инициализацию в том же канале.",
  CONFLICT: "Обновите состояние. Если создание прервалось, продолжите сохранённую операцию в существующем канале.",
  NO_VAULT: "Проверьте binding в Settings. Подключите канал или повторите синхронизацию; при отсутствии snapshot продолжите инициализацию.",
  READONLY: "Облачные изменения доступны только после ONLINE_READY. Локальная библиотека и кэш остаются доступны.",
  NETWORK: "Дождитесь соединения Telegram и повторите синхронизацию.",
  OFFLINE: "Можно читать библиотеку и слушать сохранённый кэш. Повторите синхронизацию после подключения.",
  AUTH_REQUIRED: "Откройте Settings и завершите вход в Telegram.",
  CANCELLED: "Операция остановлена. Подтверждённые данные сохранены; проверьте Imports перед повторной отправкой.",
  DATABASE_CORRUPT: "В Settings доступно восстановление из проверенного snapshot с сохранением исходной SQLite.",
  NATIVE_UNAVAILABLE: "Укажите закреплённую TDLib 1.8.69 через KIYORA_TDJSON и перезапустите приложение.",
  PROFILE_BUSY: "Этот профиль занят другим процессом. Закройте использующий его CLI или второй экземпляр Kiyora.",
  SECURE_STORAGE_UNAVAILABLE: "Разблокируйте системное хранилище секретов и повторите подключение.",
  TIMEOUT: "Исход отправки может быть неизвестен. Возобновите существующую операцию, сохранив её идентификатор.",
};
export function duration(ms = 0) { const seconds = Math.floor(ms / 1000); return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}`; }
export const uuid = () => crypto.randomUUID();
