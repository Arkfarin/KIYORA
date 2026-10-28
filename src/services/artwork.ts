import { call } from "../backend";
import type { Artwork } from "../backend";
type Cached = {
  url: string;
  users: number;
  touched: number;
  tone?: Promise<string | undefined>;
};
const entries = new Map<string, Cached>();
const pending = new Map<string, Promise<Cached | null>>();
let running = 0;
const waiting: (() => void)[] = [];
function toneFor(asset: Cached) {
  // One small sample per cached cover, requested only by Now Playing.
  // Browser image decoding is asynchronous; no full-size pixel buffer in React.
  asset.tone ??= (async () => {
    const picture = new Image();
    picture.src = asset.url;
    await picture.decode();
    const canvas = document.createElement("canvas");
    canvas.width = canvas.height = 24;
    const ctx = canvas.getContext("2d", { willReadFrequently: true });
    if (!ctx) return undefined;
    ctx.drawImage(picture, 0, 0, 24, 24);
    const pixels = ctx.getImageData(0, 0, 24, 24).data;
    let red = 0,
      green = 0,
      blue = 0,
      weight = 0;
    for (let i = 0; i < pixels.length; i += 4) {
      const alpha = pixels[i + 3] / 255;
      red += pixels[i] * alpha;
      green += pixels[i + 1] * alpha;
      blue += pixels[i + 2] * alpha;
      weight += alpha;
    }
    return weight
      ? `rgb(${Math.round(red / weight)} ${Math.round(green / weight)} ${Math.round(blue / weight)})`
      : undefined;
  })().catch(() => undefined);
  return asset.tone;
}
async function limited<T>(fn: () => Promise<T>): Promise<T> {
  if (running >= 4) await new Promise<void>((resolve) => waiting.push(resolve));
  running++;
  try {
    return await fn();
  } finally {
    running--;
    waiting.shift()?.();
  }
}
export function artworkUrl(art: Artwork) {
  if (
    art.bytes.length > 4 * 1024 * 1024 ||
    !["image/png", "image/jpeg", "image/webp"].includes(art.mime_type)
  )
    throw new Error("Invalid cover");
  return URL.createObjectURL(
    new Blob([Uint8Array.from(art.bytes)], { type: art.mime_type }),
  );
}
function trim() {
  for (const [key, value] of [...entries].sort(
    (a, b) => a[1].touched - b[1].touched,
  )) {
    if (entries.size <= 64) break;
    if (!value.users) {
      URL.revokeObjectURL(value.url);
      entries.delete(key);
    }
  }
}
export async function acquireArtwork(
  session: string,
  track: string,
  sha: string,
  kind: "track" | "playlist" = "track",
) {
  const key = `${session}:${sha}`;
  let cached = entries.get(key);
  if (!cached) {
    let request = pending.get(key);
    if (!request) {
      request = limited(async () => {
        const art =
          kind === "playlist"
            ? await call("playlist_artwork", {
                session_id: session,
                playlist_id: track,
              })
            : await call("artwork", { session_id: session, track_id: track });
        if (!art || art.sha256 !== sha) return null;
        const value = { url: artworkUrl(art), users: 0, touched: Date.now() };
        entries.set(key, value);
        return value;
      }).finally(() => pending.delete(key));
      pending.set(key, request);
    }
    cached = (await request) ?? undefined;
  }
  if (!cached) return null;
  cached.users++;
  cached.touched = Date.now();
  trim();
  const held = cached;
  return {
    url: held.url,
    tone: () => toneFor(held),
    release: () => {
      held.users = Math.max(0, held.users - 1);
      trim();
    },
  };
}
export function releaseSessionArtwork(session: string) {
  for (const [key, value] of entries)
    if (key.startsWith(`${session}:`) && !value.users) {
      URL.revokeObjectURL(value.url);
      entries.delete(key);
    }
}
