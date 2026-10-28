import { call } from "../backend";
import type { Playback, Queue } from "../backend";

/** Resume the engine's current occurrence without replacing its queue. */
export async function resumePlayback(
  session_id: string,
  playback: Playback,
  queue: Queue | null,
) {
  if (playback.state === "playing" || playback.state === "buffering") return;
  if (playback.state === "paused") {
    await call("control_playback", { session_id, action: { kind: "resume" } });
  } else if (queue?.current_track_id && queue.position !== null) {
    await call("edit_queue", {
      session_id,
      queue_id: queue.queue_id,
      revision: queue.revision,
      action: { kind: "jump", index: queue.position },
    });
  }
}
