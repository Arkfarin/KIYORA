import type { Navigate, Route } from "../../app/navigation";

export function LibraryTabs({
  kind,
  navigate,
}: {
  kind: Route["kind"];
  navigate: Navigate;
}) {
  return (
    <nav className="library-tabs" aria-label="Разделы медиатеки">
      {(
        [
          ["library", "Треки"],
          ["albums", "Альбомы"],
          ["artists", "Исполнители"],
          ["playlists", "Плейлисты"],
          ["favorites", "Любимые"],
          ["downloads", "Загрузки"],
        ] as const
      ).map(([route, label]) => (
        <button
          key={route}
          aria-current={
            kind === route ||
            (kind === "album" && route === "albums") ||
            (kind === "artist" && route === "artists")
              ? "page"
              : undefined
          }
          onClick={() => navigate({ kind: route })}
        >
          {label}
        </button>
      ))}
    </nav>
  );
}
