import { useEffect, useRef, useState } from "react";
import { call, duration } from "../../backend";
import type { Playback } from "../../backend";
import type { Runtime } from "../../useBackend";

export function SeekControl({
  runtime,
  playback,
  durationMs,
}: {
  runtime: Runtime;
  playback: Playback;
  durationMs: number;
}) {
  const [preview, setPreview] = useState<number | null>(null);
  const [pending, setPending] = useState<number | null>(null);
  const request = useRef(0);
  const dragging = useRef(false);
  const session_id = runtime.model.status!.context.session_id;
  const position = playback.position_ms ?? 0;
  const available =
    ["playing", "paused", "buffering"].includes(playback.state) &&
    durationMs > 0;
  useEffect(() => {
    request.current++;
    dragging.current = false;
    setPreview(null);
    setPending(null);
  }, [
    session_id,
    playback.track_id,
    runtime.model.queue?.queue_id,
    runtime.model.queue?.position,
  ]);
  useEffect(() => {
    if (pending !== null && Math.abs(position - pending) < 1500)
      setPending(null);
  }, [position, pending]);
  useEffect(() => {
    if (pending === null) return;
    // A failed/unsupported seek must never leave a fabricated position on screen.
    const timer = setTimeout(() => {
      setPending(null);
      void runtime.refresh();
    }, 3000);
    return () => clearTimeout(timer);
  }, [pending, runtime.refresh]);
  const commit = async (value: number) => {
    if (!available) return;
    const ticket = ++request.current;
    setPreview(null);
    setPending(value);
    const ok = await runtime.run(
      "Перемотка",
      async () => {
        await call("control_playback", {
          session_id,
          action: { kind: "seek", value },
        });
        return true;
      },
      false,
    );
    if (!ok && ticket === request.current) setPending(null);
  };
  const shown = Math.max(
    0,
    Math.min(preview ?? pending ?? position, durationMs),
  );
  return (
    <div className="seek-control">
      <span>{duration(shown)}</span>
      <input
        type="range"
        aria-label="Позиция воспроизведения"
        aria-valuetext={`${duration(shown)} из ${duration(durationMs)}`}
        min={0}
        max={Math.max(1, durationMs)}
        step={1000}
        value={shown}
        disabled={!available}
        onPointerDown={(e) => {
          dragging.current = true;
          e.currentTarget.setPointerCapture(e.pointerId);
        }}
        onChange={(e) => setPreview(Number(e.target.value))}
        onPointerUp={(e) => {
          dragging.current = false;
          void commit(Number(e.currentTarget.value));
        }}
        onPointerCancel={() => {
          dragging.current = false;
          setPreview(null);
        }}
        onKeyUp={(e) => {
          if (
            [
              "ArrowLeft",
              "ArrowRight",
              "ArrowUp",
              "ArrowDown",
              "Home",
              "End",
              "PageUp",
              "PageDown",
            ].includes(e.key)
          )
            void commit(Number(e.currentTarget.value));
        }}
        onBlur={() => {
          if (!dragging.current) setPreview(null);
        }}
      />
      <span>{duration(durationMs)}</span>
    </div>
  );
}
