import {
  Pause,
  Play,
  Repeat,
  Repeat1,
  Shuffle,
  SkipBack,
  SkipForward,
} from "lucide-react";
import type {
  Playback,
  PlaybackAction,
  Queue,
  QueueAction,
} from "../../backend";
import { IconButton } from "../../components/ui/Primitives";

export function Transport({
  playback,
  queue,
  busy,
  canPlay,
  control,
  edit,
  toggle,
}: {
  playback: Playback;
  queue: Queue | null;
  busy: boolean;
  canPlay: boolean;
  control: (action: PlaybackAction) => void;
  edit: (action: QueueAction) => void;
  toggle: () => void;
}) {
  const repeat = queue?.repeat ?? "off";
  const playing =
    playback.state === "playing" || playback.state === "buffering";
  return (
    <div className="transport-buttons">
      <IconButton
        label={
          queue?.shuffle ? "Выключить случайный порядок" : "Случайный порядок"
        }
        aria-pressed={queue?.shuffle ?? false}
        disabled={!queue?.total || busy}
        onClick={() => edit({ kind: "shuffle", enabled: !queue?.shuffle })}
      >
        <Shuffle />
      </IconButton>
      <IconButton
        label="Предыдущая композиция"
        disabled={!queue?.total}
        onClick={() => control({ kind: "previous" })}
      >
        <SkipBack fill="currentColor" />
      </IconButton>
      <IconButton
        className="play-button"
        label={playing ? "Пауза" : "Слушать"}
        disabled={!canPlay}
        onClick={toggle}
      >
        {playing ? <Pause fill="currentColor" /> : <Play fill="currentColor" />}
      </IconButton>
      <IconButton
        label="Следующая композиция"
        disabled={!queue?.total}
        onClick={() => control({ kind: "next" })}
      >
        <SkipForward fill="currentColor" />
      </IconButton>
      <IconButton
        label={`Повтор: ${repeat === "off" ? "выключен" : repeat === "track" ? "композиции" : "очереди"}`}
        aria-pressed={repeat !== "off"}
        disabled={!queue?.total || busy}
        onClick={() =>
          edit({
            kind: "repeat",
            mode:
              repeat === "off" ? "queue" : repeat === "queue" ? "track" : "off",
          })
        }
      >
        {repeat === "track" ? <Repeat1 /> : <Repeat />}
      </IconButton>
    </div>
  );
}
