import { useEffect, useRef } from "react";
import type { Navigate } from "../app/navigation";
import type { Runtime } from "../useBackend";
import { call } from "../backend";
import { resumePlayback } from "../services/playback";

export function useKeyboardShortcuts(runtime: Runtime, navigate: Navigate) {
  const current = useRef({ runtime, navigate });
  current.current = { runtime, navigate };
  useEffect(() => {
    const key = (event: KeyboardEvent) => {
      if (
        event.defaultPrevented ||
        event.repeat ||
        event.altKey ||
        document.querySelector("dialog[open]:not(.player-dialog)")
      )
        return;
      const target = event.target;
      if (
        !(target instanceof HTMLElement) ||
        target.closest("input,textarea,select,[contenteditable=true]")
      )
        return;
      const { runtime, navigate } = current.current;
      const session_id = runtime.model.status?.context.session_id;
      if (!session_id) return;
      if (event.ctrlKey || event.metaKey) {
        if (
          event.key.toLowerCase() === "k" ||
          event.key.toLowerCase() === "l"
        ) {
          event.preventDefault();
          navigate({
            kind: event.key.toLowerCase() === "k" ? "search" : "library",
          });
        } else if (event.key === "ArrowRight" || event.key === "ArrowLeft") {
          event.preventDefault();
          void runtime.run(
            "Сменить композицию",
            () =>
              call("control_playback", {
                session_id,
                action: {
                  kind: event.key === "ArrowRight" ? "next" : "previous",
                },
              }),
            false,
          );
        }
      } else if (event.code === "Space" && !target.closest("button,a")) {
        event.preventDefault();
        const playback =
          runtime.playbackStore?.getSnapshot() ?? runtime.model.playback;
        void runtime.run(
          "Пауза / воспроизведение",
          async () => {
            if (playback.state === "playing" || playback.state === "buffering")
              await call("control_playback", {
                session_id,
                action: { kind: "pause" },
              });
            else
              await resumePlayback(session_id, playback, runtime.model.queue);
          },
          false,
        );
      }
    };
    window.addEventListener("keydown", key);
    return () => window.removeEventListener("keydown", key);
  }, []);
}
