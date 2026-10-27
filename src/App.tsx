import { useEffect, useRef, useState } from "react";
import { duration, errorHelp } from "./backend";
import { Authorization, Profiles, VaultSetup, phaseNames } from "./Setup";
import { Imports, Library, Player, Playlists } from "./Music";
import { useBackend } from "./useBackend";
import type { Runtime } from "./useBackend";
import "./App.css";

type Section = "library" | "playlists" | "imports" | "settings" | "diagnostics";
const sections: { id: Section; name: string; symbol: string }[] = [
  { id: "library", name: "Library", symbol: "♫" }, { id: "playlists", name: "Playlists", symbol: "≡" },
  { id: "imports", name: "Imports", symbol: "↓" }, { id: "settings", name: "Settings", symbol: "⚙" },
];
function Diagnostics({ runtime }: { runtime: Runtime }) {
  const { model } = runtime;
  const [copyState, setCopyState] = useState("");
  // This is an allowlist. Never serialize the complete model: Auth contains a
  // temporary QR login token, and UI forms can contain credentials and paths.
  const report = {
    runtime: model.status,
    profile_id: model.status?.context.profile_id,
    session_id: model.status?.context.session_id,
    vault_id: model.status?.context.vault_id,
    chat_id: model.info?.chat_id,
    tdlib: { authorization: model.auth?.state, connection: model.info?.telegram_connection },
    sync: { service_running: model.info?.service_running, progress: model.progress, repair_required: model.info?.repair_required },
    creation: model.info?.creation,
    last_error: model.error,
    playback: model.playback,
    queue: model.queue,
    imports: model.imports.map(item => ({ op_id: item.op_id, stage: item.stage })),
    jobs: model.jobs.map(job => ({ job_id: job.job_id, state: job.state, completed: job.completed, total: job.total, error: job.error })),
    events: model.info?.diagnostics,
  };
  const text = JSON.stringify(report, null, 2);
  return <>
    <div className="page-title"><div><h1>Diagnostics</h1><p className="muted">Состояние Rust runtime и последние диагностические события.</p></div><button onClick={() => {
      void navigator.clipboard.writeText(text).then(() => setCopyState("Скопировано")).catch(() => setCopyState("Clipboard недоступен. Выделите текст ниже и скопируйте вручную."));
    }}>Copy diagnostics</button></div>
    <p role="status" className="muted small">{copyState || "Экспорт исключает API hash, коды, пароли, QR-токены и содержимое secure storage."}</p>
    <pre className="diagnostics" tabIndex={0}>{text}</pre>
  </>;
}
function App() {
  const runtime = useBackend();
  const { model, busy, refresh, clearError } = runtime;
  const [section, setSection] = useState<Section>("library");
  const routedSession = useRef<string | undefined>(undefined);
  const readySession = useRef<string | undefined>(undefined);
  useEffect(() => {
    const id = model.status?.context.session_id;
    if (id && model.auth && routedSession.current !== id) {
      routedSession.current = id;
      setSection(model.auth.state === "READY" && model.status?.context.vault_id ? "library" : "settings");
    }
  }, [model.auth, model.status]);
  useEffect(() => {
    const id = model.status?.context.session_id;
    if (id && model.status?.mode === "ONLINE_READY" && readySession.current !== id) {
      readySession.current = id;
      setSection(previous => previous === "settings" ? "library" : previous);
    }
  }, [model.status]);
  const profile = model.profiles.find(p => p.profile_id === model.status?.context.profile_id);
  const connected = model.info?.telegram_connection === "connectionStateReady";
  const online = model.status?.mode === "ONLINE_READY";
  const mode = model.status?.mode ?? "NO_PROFILE";
  return <div className="app-shell">
    <aside className="sidebar">
      <div className="brand"><svg width="28" height="28" viewBox="0 0 28 28" aria-hidden="true"><path d="M5 7v14M11 3v22M17 8v12M23 11v6" stroke="currentColor" strokeWidth="3" strokeLinecap="round" /></svg><span>KIYORA</span></div>
      <nav aria-label="Разделы">{sections.map(item => <button key={item.id} className={section === item.id || (item.id === "settings" && section === "diagnostics") ? "active" : ""} onClick={() => setSection(item.id)}><span aria-hidden="true">{item.symbol}</span>{item.name}</button>)}</nav>
      <div className="sidebar-foot"><span className={`dot ${online ? "good" : ""}`} /> Personal music vault</div>
    </aside>
    <div className="workspace">
      <header className="status-bar">
        <span className="profile-name">{profile?.label ?? "Нет профиля"}</span>
        <span className="status-item"><i className={`dot ${connected ? "good" : ""}`} />Telegram: {connected ? "Connected" : model.auth?.state === "READY" ? "Connecting / Offline" : "Auth required"}</span>
        <span className="status-item">Vault: {online ? "Ready" : model.info?.service_running ? "Sync Engine active" : model.status?.context.vault_id ? "Binding saved" : "Not attached"}</span>
        <span className={`badge ${online ? "good" : ""}`}>{mode}</span>
        <button className="quiet small" onClick={() => void refresh()} disabled={model.loading}>Обновить</button>
      </header>
      <main>
        {model.error && <div className="error-banner" role="alert"><div><strong>{model.error.code}</strong><p>{model.error.message}</p><p className="small">{errorHelp[model.error.code] ?? "Посмотрите Diagnostics и повторите действие после устранения причины."}{model.error.retry_after_seconds ? ` Повтор разрешён через ${model.error.retry_after_seconds} с.` : ""}</p></div><div className="actions"><button onClick={() => setSection("settings")}>Settings</button><button className="quiet" onClick={clearError} aria-label="Закрыть сообщение">×</button></div></div>}
        {busy && <p className="activity" role="status">{busy}{model.progress ? ` · ${phaseNames[model.progress.phase] ?? model.progress.phase}` : "…"}</p>}
        {model.loading ? <div className="empty">Восстановление активного профиля…</div> : !model.status ? <div className="onboarding"><h1>Добро пожаловать в Kiyora</h1><p className="muted">Музыка в вашем Telegram. Создайте профиль или откройте существующий.</p><Profiles runtime={runtime} /></div> : <>
          {!online && <div className="readonly"><span>{mode === "SYNCING" ? "Синхронизация: облачные изменения временно заблокированы." : "Облачные изменения доступны после ONLINE_READY."} Локальная библиотека и сохранённое аудио доступны.</span><button className="quiet small" onClick={() => setSection("settings")}>Подключение</button></div>}
          {section === "library" && <Library runtime={runtime} importFiles={() => setSection("imports")} />}
          {section === "playlists" && <Playlists runtime={runtime} />}
          {section === "imports" && <Imports runtime={runtime} />}
          {section === "settings" && <>
            <div className="page-title"><div><h1>Settings</h1><p className="muted">Профиль, авторизация и подключение хранилища.</p></div><button onClick={() => setSection("diagnostics")}>Debug / Diagnostics</button></div>
            <div className="settings-grid"><div><Profiles runtime={runtime} /><Authorization key={model.status.context.session_id} runtime={runtime} /></div><VaultSetup key={model.status.context.session_id} runtime={runtime} /></div>
            <p className="small muted">Плеер: {model.playback.state} · {duration(model.playback.position_ms)}. Сессия и Sync Engine работают в Rust независимо от открытого раздела.</p>
          </>}
          {section === "diagnostics" && <Diagnostics runtime={runtime} />}
        </>}
      </main>
    </div>
    {model.status && <Player key={model.status.context.session_id} runtime={runtime} />}
  </div>;
}
export default App;
