import { memo, useEffect, useRef, useState } from "react";
import { Disc3 } from "lucide-react";
import type { Playlist, Track } from "../../backend";
import { acquireArtwork } from "../../services/artwork";
export const Cover = memo(function Cover({
  track,
  playlist,
  session,
  className = "",
}: {
  track?: Track;
  playlist?: Playlist;
  session: string;
  className?: string;
}) {
  const ref = useRef<HTMLSpanElement>(null);
  const [url, setUrl] = useState<string>();
  const sha = playlist?.artwork?.sha256 ?? track?.metadata.artwork?.sha256;
  const id = playlist?.playlist_id ?? track?.track_id;
  const kind = playlist ? "playlist" : "track";
  useEffect(() => {
    let disposed = false;
    let release: (() => void) | undefined;
    let started = false;
    setUrl(undefined);
    const load = () => {
      if (!sha || !id || started) return;
      started = true;
      void acquireArtwork(session, id, sha, kind)
        .then((asset) => {
          if (disposed) asset?.release();
          else if (asset) {
            release = asset.release;
            setUrl(asset.url);
          }
        })
        .catch(() => {
          /* Optional art keeps the accessible fallback. */
        });
    };
    const observer = new IntersectionObserver(
      (items) => {
        if (items.some((i) => i.isIntersecting)) {
          load();
          observer.disconnect();
        }
      },
      { rootMargin: "160px" },
    );
    if (ref.current) observer.observe(ref.current);
    return () => {
      disposed = true;
      observer.disconnect();
      release?.();
    };
  }, [session, id, sha, kind]);
  return (
    <span ref={ref} className={`cover ${className}`} aria-hidden="true">
      {url ? (
        <img
          src={url}
          alt=""
          draggable={false}
          loading="lazy"
          decoding="async"
          onError={() => setUrl(undefined)}
        />
      ) : (
        <Disc3 strokeWidth={1} />
      )}
    </span>
  );
});
