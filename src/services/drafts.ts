import type { LyricsDocument, MediaEdit, MetadataPatch } from "../backend";
export interface MetadataDraft {
  patch: MetadataPatch;
  lyrics: LyricsDocument | null;
  lyricsChanged: boolean;
  artwork: number[] | null;
  artworkChanged: boolean;
  order?: string;
  savedAt: number;
  opId: string;
  writeTags?: boolean;
  attempt?: { edit: MediaEdit; writeTags: boolean };
}
const writes = new Map<string, Promise<void>>();
function database() {
  return new Promise<IDBDatabase>((resolve, reject) => {
    const request = indexedDB.open("kiyora-editor", 1);
    request.onupgradeneeded = () => request.result.createObjectStore("drafts");
    request.onerror = () => reject(request.error);
    request.onsuccess = () => resolve(request.result);
  });
}
export async function readDraft(
  key: string,
): Promise<MetadataDraft | undefined> {
  const db = await database();
  try {
    return await new Promise((resolve, reject) => {
      const tx = db.transaction("drafts");
      const req = tx.objectStore("drafts").get(key);
      req.onsuccess = () => resolve(req.result as MetadataDraft | undefined);
      req.onerror = () => reject(req.error);
    });
  } finally {
    db.close();
  }
}
async function persistDraft(key: string, value?: MetadataDraft) {
  const db = await database();
  try {
    await new Promise<void>((resolve, reject) => {
      const tx = db.transaction("drafts", "readwrite");
      const store = tx.objectStore("drafts");
      if (value) store.put(value, key);
      else store.delete(key);
      tx.oncomplete = () => resolve();
      tx.onerror = () => reject(tx.error);
      tx.onabort = () => reject(tx.error);
    });
  } finally {
    db.close();
  }
}
// A delayed autosave must never resurrect a draft after successful publication.
export function writeDraft(key: string, value?: MetadataDraft) {
  const next = (writes.get(key) ?? Promise.resolve())
    .catch(() => {})
    .then(() => persistDraft(key, value));
  writes.set(key, next);
  void next
    .finally(() => {
      if (writes.get(key) === next) writes.delete(key);
    })
    .catch(() => {});
  return next;
}
