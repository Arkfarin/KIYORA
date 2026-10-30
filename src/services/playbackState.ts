import { useSyncExternalStore } from "react";
import type { Playback } from "../backend";
import type { Runtime } from "../useBackend";

/** A subscription to Rust snapshots, not a second playback clock or engine. */
export function createPlaybackStore(initial: Playback) {
  let value = initial;
  const listeners = new Set<() => void>();
  return {
    getSnapshot: () => value,
    subscribe: (listener: () => void) => {
      listeners.add(listener);
      return () => {
        listeners.delete(listener);
      };
    },
    publish: (next: Playback) => {
      if (next === value) return;
      value = next;
      listeners.forEach((listener) => listener());
    },
  };
}
const noSubscription = () => () => {};
export function usePlayback(runtime: Runtime) {
  return useSyncExternalStore(
    runtime.playbackStore?.subscribe ?? noSubscription,
    runtime.playbackStore?.getSnapshot ?? (() => runtime.model.playback),
  );
}
