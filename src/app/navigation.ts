export type Route =
  | {
      kind:
        | "home"
        | "search"
        | "library"
        | "favorites"
        | "albums"
        | "artists"
        | "playlists"
        | "imports"
        | "downloads"
        | "settings";
    }
  | { kind: "album"; title: string; artist: string }
  | { kind: "artist"; name: string }
  | { kind: "playlist"; id: string };
export type Navigate = (route: Route) => void;
