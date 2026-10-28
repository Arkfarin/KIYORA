import {
  Home,
  Search,
  Library,
  Heart,
  Disc3,
  Mic2,
  ListMusic,
  Download,
  Settings,
  PanelLeftClose,
  PanelLeftOpen,
  Plus,
  ArrowUpToLine,
  Cloud,
  CloudOff,
} from "lucide-react";
import type { ReactNode } from "react";
import { useState } from "react";
import type { Runtime } from "../../useBackend";
import type { Navigate, Route } from "../../app/navigation";
import { Cover } from "../music/Cover";
import { IconButton } from "../ui/Primitives";
const destinations = [
  { kind: "home", label: "Главная", Icon: Home },
  { kind: "search", label: "Поиск", Icon: Search },
  { kind: "library", label: "Медиатека", Icon: Library },
  { kind: "favorites", label: "Любимые треки", Icon: Heart },
  { kind: "albums", label: "Альбомы", Icon: Disc3 },
  { kind: "artists", label: "Исполнители", Icon: Mic2 },
  { kind: "playlists", label: "Плейлисты", Icon: ListMusic },
  { kind: "downloads", label: "Загрузки", Icon: Download },
  { kind: "settings", label: "Настройки", Icon: Settings },
] as const;
export function Shell({
  runtime,
  route,
  navigate,
  children,
  player,
  createPlaylist,
}: {
  runtime: Runtime;
  route: Route;
  navigate: Navigate;
  children: ReactNode;
  player: ReactNode;
  createPlaylist: () => void;
}) {
  const [compact, setCompact] = useState(false);
  const { model } = runtime;
  const profile = model.profiles.find(
    (p) => p.profile_id === model.status?.context.profile_id,
  );
  const online = model.status?.mode === "ONLINE_READY";
  const status = online
    ? "Синхронизировано"
    : model.status?.mode === "SYNCING"
      ? "Синхронизация"
      : model.status?.mode === "DEGRADED_READONLY"
        ? "Только чтение"
        : "Без подключения";
  const playlists = Object.values(model.projection?.state.playlists ?? {});
  return (
    <div className={`app-shell ${compact ? "sidebar-compact" : ""}`}>
      <a className="skip-link" href="#main-content">
        К содержимому
      </a>
      <aside className="sidebar" aria-label="Главная навигация">
        <div className="sidebar-brand">
          <button
            className="wordmark"
            onClick={() => navigate({ kind: "home" })}
          >
            KIYORA
          </button>
          <IconButton
            label={compact ? "Развернуть навигацию" : "Свернуть навигацию"}
            onClick={() => setCompact((v) => !v)}
          >
            {compact ? <PanelLeftOpen /> : <PanelLeftClose />}
          </IconButton>
        </div>
        <nav>
          {destinations.map(({ kind, label, Icon }) => (
            <button
              key={kind}
              title={label}
              aria-current={route.kind === kind ? "page" : undefined}
              onClick={() => navigate({ kind })}
            >
              <Icon />
              <span>{label}</span>
            </button>
          ))}
        </nav>
        <div className="sidebar-playlists">
          <div className="section-heading">
            <span>Ваша коллекция</span>
            <IconButton
              label="Создать плейлист"
              disabled={!online}
              onClick={createPlaylist}
            >
              <Plus />
            </IconButton>
          </div>
          <div className="playlist-shortcuts">
            {playlists.map((p) => (
              <button
                key={p.playlist_id}
                title={p.title}
                onClick={() =>
                  navigate({ kind: "playlist", id: p.playlist_id })
                }
                aria-current={
                  route.kind === "playlist" && route.id === p.playlist_id
                    ? "page"
                    : undefined
                }
              >
                {p.artwork ? (
                  <Cover
                    session={model.status!.context.session_id}
                    playlist={p}
                  />
                ) : (
                  <ListMusic />
                )}
                <span>{p.title}</span>
              </button>
            ))}
            {!playlists.length && (
              <p className="muted small">Соберите музыку для любого момента.</p>
            )}
          </div>
        </div>
        <button
          className="profile-switch"
          onClick={() => navigate({ kind: "settings" })}
        >
          <span className="profile-initial">
            {profile?.label.slice(0, 1).toLocaleUpperCase() || "K"}
          </span>
          <span>
            <strong>{profile?.label || "Ваш профиль"}</strong>
            <small>Личное музыкальное облако</small>
          </span>
        </button>
      </aside>
      <div className="workspace">
        <header className="topbar">
          <button
            className="wordmark mobile-wordmark"
            onClick={() => navigate({ kind: "home" })}
          >
            KIYORA
          </button>
          <button
            className="search-shortcut"
            onClick={() => navigate({ kind: "search" })}
          >
            <Search />
            <span>Найти свою музыку</span>
            <kbd>Ctrl K</kbd>
          </button>
          <div className="topbar-actions">
            <button
              className={`connection ${online ? "connected" : ""}`}
              onClick={() => navigate({ kind: "settings" })}
            >
              {online ? <Cloud /> : <CloudOff />}
              <span>{status}</span>
            </button>
            <IconButton
              label="Добавить музыку"
              disabled={!online}
              onClick={() => navigate({ kind: "imports" })}
            >
              <Plus />
            </IconButton>
          </div>
        </header>
        <main id="main-content" tabIndex={-1}>
          {!online && (
            <div className="connection-notice" role="status">
              {model.status?.mode === "SYNCING"
                ? "Обновляем библиотеку. Пока можно слушать сохранённую музыку."
                : "Локальная библиотека доступна. Изменения появятся после подключения."}
            </div>
          )}
          {children}
        </main>
      </div>
      {player}
      <nav className="mobile-nav" aria-label="Навигация телефона">
        {destinations
          .filter((d) =>
            ["home", "search", "library", "settings"].includes(d.kind),
          )
          .map(({ kind, label, Icon }) => (
            <button
              key={kind}
              aria-current={route.kind === kind ? "page" : undefined}
              onClick={() => navigate({ kind })}
            >
              <Icon />
              <span>{label}</span>
            </button>
          ))}
      </nav>
      <button
        className="import-mobile sr-only"
        onClick={() => navigate({ kind: "imports" })}
      >
        <ArrowUpToLine />
        Импорт
      </button>
    </div>
  );
}
