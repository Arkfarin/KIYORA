import { call } from "../backend";
import type { Artwork } from "../backend";
type Cached = { url: string; users: number; touched: number };
const entries = new Map<string, Cached>();
const pending = new Map<string, Promise<Cached | null>>();
let running = 0;
const waiting: (() => void)[] = [];
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
) {
  const key = `${session}:${sha}`;
  let cached = entries.get(key);
  if (!cached) {
    let request = pending.get(key);
    if (!request) {
      request = limited(async () => {
        const art = await call("artwork", {
          session_id: session,
          track_id: track,
        });
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
