import { useEffect, useState } from "react";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { call, duration, errorOf, resumeJob, uuid } from "./backend";
import type { Command, Download, MutationAction, PlaylistDetail, Track, VaultRequest } from "./backend";
import type { Runtime } from "./useBackend";

export function Cover({ track, session }: { track?: Track; session: string }) {
  const [url, setUrl] = useState<string>();
  useEffect(() => {
    let active = true;
    let objectUrl: string | undefined;
    setUrl(undefined);
    if (track?.metadata.artwork) void call("artwork", { session_id: session, track_id: track.track_id }).then(art => {
      if (!active || !art || !["image/jpeg", "image/png", "image/webp"].includes(art.mime_type)) return;
      objectUrl = URL.createObjectURL(new Blob([Uint8Array.from(art.bytes)], { type: art.mime_type }));
      setUrl(objectUrl);
    }).catch(() => { /* Artwork is optional; audio errors are surfaced by Play. */ });
    return () => { active = false; if (objectUrl) URL.revokeObjectURL(objectUrl); };
  }, [session, track?.track_id, track?.metadata.artwork?.sha256]);
  return url ? <img className="cover" src={url} alt="" /> : <span className="cover placeholder" aria-hidden="true">♫</span>;
}
function TrackCache({ track, runtime }: { track: Track; runtime: Runtime }) {
  const { model, busy, run } = runtime;
  const [cached, setCached] = useState<Download | null>(null);
  const session_id = model.status!.context.session_id;
  const live = model.downloads[track.track_id];
  useEffect(() => {
    let active = true;
    setCached(null);
    void (async () => {
      let best: Download | null = null;
      for (const source_id of Object.keys(track.sources)) {
        const response = await call("vault", { session_id, request: { kind: "get_download_status", payload: { download_id: track.track_id, track_id: track.track_id, source_id } } });
        const value = response.result as Download;
        best = value;
        if (["fully_cached", "partially_cached", "downloading"].includes(value.state)) break;
      }
      if (active) setCached(best);
    })().catch(() => { if (active) setCached(null); });
    return () => { active = false; };
  }, [session_id, track, live?.state]);
  const value = live ?? cached;
  const labels: Record<string, string> = { fully_cached: "В кэше", partially_cached: "Частичный кэш", downloading: "Загрузка", remote_only: "В облаке", failed_retryable: "Ошибка загрузки", failed_permanent: "Источник недоступен", cancelled: "Отменено", unavailable: "Недоступен" };
  const source = Object.values(track.sources)[0];
  return <div className="cache-state"><span className="small muted">{value ? labels[value.state] ?? value.state : "Кэш: —"}</span>
    {source && value?.state !== "fully_cached" && <button className="small" disabled={!!busy || model.status?.mode !== "ONLINE_READY" || value?.state === "downloading"} onClick={() => void run("Загрузка для offline", () => call("vault", { session_id, request: { kind: "download_for_offline", payload: { download_id: uuid(), track_id: track.track_id, source_id: source.source_id, pinned: true } } }))}>Сохранить</button>}
  </div>;
}

export function Library({ runtime, importFiles }: { runtime: Runtime; importFiles: () => void }) {
  const { model, busy, run } = runtime;
  const [query, setQuery] = useState("");
  const [page, setPage] = useState(0);
  const session_id = model.status!.context.session_id;
  const tracks = Object.values(model.projection?.state.tracks ?? {}).filter(t => `${t.metadata.title} ${t.metadata.performer} ${t.metadata.album}`.toLocaleLowerCase().includes(query.toLocaleLowerCase())).sort((a, b) => a.metadata.title.localeCompare(b.metadata.title));
  const visible = tracks.slice(page * 40, (page + 1) * 40);
  useEffect(() => { setPage(0); }, [query, session_id]);
  return <>
    <div className="page-title"><div><h1>Library</h1><p className="muted">{tracks.length} треков в локальной библиотеке</p></div><button className="primary" onClick={importFiles} disabled={model.status?.mode !== "ONLINE_READY"}>Импорт аудио</button></div>
    <label className="search"><span className="sr-only">Поиск по библиотеке</span><input value={query} onChange={e => setQuery(e.target.value)} placeholder="Название, исполнитель, альбом" /></label>
    {visible.length === 0 ? <div className="empty">{model.projection ? "Здесь пока нет треков. Импортируйте аудиофайл, когда Vault готов." : "Подключите Vault в Settings. Сохранённая библиотека появится после загрузки SQLite."}</div> : <div className="track-list">{visible.map(track => <div className="track-row" key={track.track_id}>
      <Cover track={track} session={session_id} />
      <button className="track-name" disabled={!!busy} onClick={() => void run("Воспроизведение", () => call("set_queue", { session_id, track_ids: tracks.map(t => t.track_id), position: tracks.indexOf(track), autoplay: true }))}>
        <strong>{track.metadata.title || "Без названия"}</strong><span>{track.metadata.performer || "Без исполнителя"}</span>
      </button>
      <span className="muted small track-duration">{duration(track.metadata.duration_ms)}</span>
      <TrackCache track={track} runtime={runtime} />
    </div>)}</div>}
    <div className="pagination"><button disabled={page === 0} onClick={() => setPage(p => p - 1)}>←</button><span>{page + 1} / {Math.max(1, Math.ceil(tracks.length / 40))}</span><button disabled={(page + 1) * 40 >= tracks.length} onClick={() => setPage(p => p + 1)}>→</button></div>
  </>;
}

export function Playlists({ runtime }: { runtime: Runtime }) {
  const { model, busy, run } = runtime;
  const [title, setTitle] = useState("");
  const [selected, setSelected] = useState("");
  const [detail, setDetail] = useState<PlaylistDetail | null>(null);
  const [loadError, setLoadError] = useState("");
  const [trackId, setTrackId] = useState("");
  const [offset, setOffset] = useState(0);
  const session_id = model.status!.context.session_id;
  const online = model.status?.mode === "ONLINE_READY";
  const playlists = Object.values(model.projection?.state.playlists ?? {});
  const tracks = Object.values(model.projection?.state.tracks ?? {});
  useEffect(() => {
    let active = true;
    setLoadError(""); setDetail(null);
    if (selected) void call("query", { session_id, query: { kind: "playlist", payload: { playlist_id: selected, offset, limit: 100 } } }).then(result => {
      if (active) setDetail(result.result as PlaylistDetail | null);
    }).catch(error => { if (active) setLoadError(errorOf(error).message); });
    return () => { active = false; };
  }, [session_id, selected, offset, model.projection]);
  const mutate = async (entity_id: string, action: MutationAction) => {
    const response = await call("vault", { session_id, request: { kind: "mutate", payload: { op_id: uuid(), entity_id, action } } });
    if (response.kind === "command") {
      const command = response.result as Command;
      if (command.status !== "APPLIED") throw { code: "CONFLICT", message: `Команда: ${command.status}. Обновите плейлист перед повтором.`, retryable: false };
    }
  };
  return <>
    <div className="page-title"><div><h1>Playlists</h1><p className="muted">Общая библиотека · изменения только online</p></div></div>
    <form className="inline-form" onSubmit={event => { event.preventDefault(); void run("Создание плейлиста", async () => { const id = uuid(); await mutate(id, { action: "playlist.create", payload: { title: title.trim() } }); setTitle(""); setSelected(id); setOffset(0); }); }}>
      <label>Название<input value={title} onChange={e => setTitle(e.target.value)} required maxLength={512} /></label><button className="primary" disabled={!online || !!busy || !title.trim()}>Создать</button>
    </form>
    <div className="playlist-layout"><div className="playlist-menu">{playlists.length === 0 && <p className="muted">Плейлистов пока нет.</p>}{playlists.map(p => <button className={selected === p.playlist_id ? "selected" : ""} key={p.playlist_id} onClick={() => { setSelected(p.playlist_id); setOffset(0); }}>{p.title}</button>)}</div>
      <section className="panel">
        {loadError && <p role="alert">{loadError}</p>}
        {!selected ? <p className="muted">Выберите плейлист.</p> : !detail ? <p className="muted">Загрузка плейлиста…</p> : <>
          <div className="section-title"><h2>{detail.playlist.title}</h2><button disabled={detail.total === 0 || !!busy} onClick={() => void run("Воспроизведение плейлиста", () => call("queue_playlist", { session_id, playlist_id: selected, position: 0, autoplay: true }))}>▶ Слушать</button></div>
          <form className="inline-form" onSubmit={event => { event.preventDefault(); void run("Добавление трека", () => mutate(selected, { action: "playlist.track.add", payload: { track_id: trackId, entry_id: uuid(), after_entry_id: null } })); }}>
            <label>Добавить в начало<select value={trackId} onChange={e => setTrackId(e.target.value)}><option value="">Выберите трек</option>{tracks.map(t => <option key={t.track_id} value={t.track_id}>{t.metadata.title} — {t.metadata.performer}</option>)}</select></label><button disabled={!online || !!busy || !trackId}>Добавить</button>
          </form>
          {detail.items.map((item, index) => <div className="playlist-track" key={item.entry.entry_id}>
            <button className="track-name" disabled={!!busy} onClick={() => void run("Воспроизведение", () => call("queue_playlist", { session_id, playlist_id: selected, position: offset + index, autoplay: true }))}><strong>{item.track.metadata.title}</strong><span>{item.track.metadata.performer}</span></button>
            <button className="small" disabled={!online || !!busy} onClick={() => void run("Удаление из плейлиста", () => mutate(selected, { action: "playlist.track.remove", payload: { entry_id: item.entry.entry_id } }))}>Убрать</button>
          </div>)}
          {detail.total === 0 && <p className="muted">Плейлист пуст.</p>}
          {detail.total > 100 && <div className="pagination"><button disabled={offset === 0} onClick={() => setOffset(n => Math.max(0, n - 100))}>←</button><span>{offset + 1}–{Math.min(offset + 100, detail.total)} / {detail.total}</span><button disabled={offset + 100 >= detail.total} onClick={() => setOffset(n => n + 100)}>→</button></div>}
        </>}
      </section>
    </div>
  </>;
}

const importStages: Record<string, string> = { staging: "Подготовка файла", prepared: "Ожидает отправки", upload_unknown: "Исход отправки неизвестен — требуется возобновление", uploaded: "Файл отправлен — регистрация трека", complete: "Завершено" };
const jobStates: Record<string, string> = { running: "Выполняется", succeeded: "Завершено", failed: "Ошибка", interrupted: "Требуется возобновление", cancelled: "Отменено — можно возобновить импорт" };
export function Imports({ runtime }: { runtime: Runtime }) {
  const { model, run, busy } = runtime;
  const [path, setPath] = useState("");
  const [paths, setPaths] = useState<string[]>([]);
  const [allowDuplicate, setAllowDuplicate] = useState(false);
  const [dropError, setDropError] = useState("");
  const [dragging, setDragging] = useState(false);
  const session_id = model.status!.context.session_id;
  const online = model.status?.mode === "ONLINE_READY";
  useEffect(() => {
    let disposed = false;
    let off: (() => void) | undefined;
    void getCurrentWebview().onDragDropEvent(event => {
      if (disposed) return;
      setDragging(event.payload.type === "over" || event.payload.type === "enter");
      if (event.payload.type === "drop") { setPaths(previous => [...new Set([...previous, ...event.payload.paths])]); setDragging(false); }
    }).then(unlisten => { if (disposed) unlisten(); else off = unlisten; }).catch(() => setDropError("Перетаскивание недоступно; введите абсолютный путь."));
    return () => { disposed = true; off?.(); };
  }, []);
  const start = async (request: VaultRequest) => call("start_job", { session_id, job_id: uuid(), request: { kind: "vault", payload: request } });
  return <>
    <div className="page-title"><div><h1>Imports</h1><p className="muted">Операции продолжаются при смене раздела и перезагрузке интерфейса.</p></div></div>
    <section className={`panel drop-zone ${dragging ? "dragging" : ""}`}>
      <h2>Импорт аудио</h2><p>Перетащите файлы из файлового менеджера в окно или укажите абсолютный путь.</p>
      {dropError && <p className="muted">{dropError}</p>}
      <form className="inline-form" onSubmit={event => { event.preventDefault(); if (path.trim()) { setPaths(p => [...new Set([...p, path.trim()])]); setPath(""); } }}>
        <label>Путь к файлу<input value={path} onChange={e => setPath(e.target.value)} placeholder="/home/…/Music/track.flac" spellCheck={false} /></label><button disabled={!path.trim()}>Добавить файл</button>
      </form>
      {paths.map(file => <div className="pending-file" key={file}><span>{file}</span><button className="small" onClick={() => setPaths(p => p.filter(item => item !== file))}>Убрать</button></div>)}
      <label className="check"><input type="checkbox" checked={allowDuplicate} onChange={e => setAllowDuplicate(e.target.checked)} /> Разрешить дубликаты</label>
      <button className="primary" disabled={!online || !!busy || paths.length === 0} onClick={() => void run("Запуск импорта", async () => {
        for (const file of paths) {
          await start({ kind: "import", payload: { path: file, track_id: uuid(), source_id: uuid(), op_id: uuid(), allow_duplicate: allowDuplicate } });
          setPaths(p => p.filter(value => value !== file));
        }
      })}>Импортировать {paths.length || ""}</button>
      {!online && <p className="muted small">Ожидается ONLINE_READY. Выбранные файлы ещё не отправлены.</p>}
    </section>
    {model.jobs.length > 0 && <section className="panel"><h2>Фоновые операции</h2>{model.jobs.map(job => <div className="job" key={job.job_id}>
      <div><strong>{jobStates[job.state]}</strong><span className="mono small">{job.job_id}</span><span className="small muted">{job.completed} / {job.total} операций</span>{job.error && <p className="error-text">{job.error.code}: {job.error.message}</p>}
        {job.result?.kind === "vault" && job.result.result.kind === "import_review" && <p>Найден возможный дубликат. Для отдельной копии повторно выберите файл и включите «Разрешить дубликаты».</p>}
      </div>
      {["failed", "interrupted", "cancelled"].includes(job.state) && <button disabled={!online || !!busy} onClick={() => void run("Возобновление задачи", () => resumeJob(session_id, job.job_id))}>Возобновить</button>}
      {job.state === "running" && <button disabled={!!busy} onClick={() => void run("Отмена задачи", () => call("cancel_job", { session_id, job_id: job.job_id }))}>Отменить</button>}
    </div>)}</section>}
    <section className="panel"><h2>Журнал импортов</h2>{model.imports.length === 0 && <p className="muted">Импортов ещё нет.</p>}{model.imports.map(item => <div className="job" key={item.op_id}>
      <div><strong>{item.metadata.title || "Аудиофайл"}</strong><span className="muted">{importStages[item.stage]}</span><span className="mono small">{item.op_id}</span></div>
      {item.stage !== "complete" && <button disabled={!online || !!busy || model.jobs.some(job => job.state === "running")} onClick={() => void run("Возобновление импорта", () => start({ kind: "resume_import", payload: { op_id: item.op_id } }))}>Возобновить</button>}
    </div>)}<p className="muted small">Показано до 500 последних записей. Проценты загрузки отображаются только когда backend сообщает размер.</p></section>
  </>;
}

export function Player({ runtime }: { runtime: Runtime }) {
  const { model, run, busy } = runtime;
  const [volume, setVolume] = useState(1);
  const [seek, setSeek] = useState<number | null>(null);
  useEffect(() => { if (model.info) setVolume(model.info.playback_volume); }, [model.info?.playback_volume]);
  const session_id = model.status!.context.session_id;
  const playback = model.playback;
  const trackId = playback.track_id ?? model.queue?.current_track_id;
  const track = trackId ? model.projection?.state.tracks[trackId] : undefined;
  const active = ["playing", "paused", "buffering"].includes(playback.state);
  const control = (kind: "next" | "previous" | "pause" | "resume" | "stop") => void run("Управление плеером", () => call("control_playback", { session_id, action: { kind } }), false);
  return <footer className="player" aria-label="Аудиоплеер">
    <div className="now-playing"><Cover track={track} session={session_id} /><div><strong>{track?.metadata.title ?? "Выберите трек"}</strong><span className="muted small">{track?.metadata.performer ?? "Kiyora"}</span>{playback.state === "failed" && <span className="error-text small">{playback.code ?? "PLAYBACK_UNAVAILABLE"}</span>}</div></div>
    <div className="transport"><div className="player-buttons">
      <button aria-label="Предыдущий трек" disabled={!!busy || !model.queue?.total || (model.queue.position ?? 0) === 0} onClick={() => control("previous")}>|‹</button>
      <button className="primary play" aria-label={playback.state === "playing" ? "Пауза" : "Воспроизведение"} disabled={!!busy || (!active && !track)} onClick={() => {
        if (active) control(playback.state === "playing" ? "pause" : "resume");
        else if (track) void run("Воспроизведение", () => call("set_queue", { session_id, track_ids: [track.track_id], position: 0, autoplay: true }));
      }}>{playback.state === "playing" ? "Ⅱ" : "▶"}</button>
      <button aria-label="Следующий трек" disabled={!!busy || !model.queue?.total || model.queue.position === null || model.queue.position + 1 >= model.queue.total} onClick={() => control("next")}>›|</button>
      <button className="small" disabled={!active || !!busy} onClick={() => control("stop")}>Stop</button>
    </div>
      <div className="seek"><span>{duration(seek ?? playback.position_ms)}</span><input aria-label="Позиция" type="range" min="0" max={Math.max(1, playback.duration_ms ?? 1)} value={seek ?? playback.position_ms ?? 0} disabled={!active || !playback.duration_ms} onChange={e => setSeek(Number(e.target.value))} onPointerUp={e => {
        const value = Number(e.currentTarget.value); setSeek(null); void run("Перемотка", () => call("control_playback", { session_id, action: { kind: "seek", value } }), false);
      }} onKeyUp={e => { if (["ArrowLeft", "ArrowRight", "Home", "End"].includes(e.key)) { const value = Number(e.currentTarget.value); setSeek(null); void run("Перемотка", () => call("control_playback", { session_id, action: { kind: "seek", value } }), false); } }} /><span>{duration(playback.duration_ms)}</span></div>
    </div>
    <label className="volume">Volume<input type="range" min="0" max="1" step="0.02" value={volume} disabled={!active} onChange={e => { const value = Number(e.target.value); setVolume(value); void run("Громкость", () => call("control_playback", { session_id, action: { kind: "set_volume", value } }), false); }} /></label>
  </footer>;
}
