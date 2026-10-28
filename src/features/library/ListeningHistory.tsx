import { useEffect, useState } from "react";
import { call, errorOf } from "../../backend";
import type { Track } from "../../backend";
import type { Runtime } from "../../useBackend";
import type { Navigate } from "../../app/navigation";
import { TrackList } from "../../components/music/TrackList";

export function ListeningHistory(props: {
  runtime: Runtime;
  navigate: Navigate;
  onMenu: (track: Track) => void;
  onDelete: (tracks: Track[]) => void;
}) {
  const { model } = props.runtime;
  const [history, setHistory] = useState<
    { track_id: string; completed_plays: number; last_played_ms: number }[]
  >([]);
  const [error, setError] = useState("");
  const session_id = model.status!.context.session_id;
  useEffect(() => {
    let active = true;
    void call("listening_history", { session_id })
      .then((value) => {
        if (active) {
          setHistory(value);
          setError("");
        }
      })
      .catch((e) => {
        if (active) setError(errorOf(e).message);
      });
    return () => {
      active = false;
    };
  }, [session_id, model.queue?.current_track_id, model.revision]);
  const available = history.filter(
    (item) => model.projection?.state.tracks[item.track_id],
  );
  const recent = available
    .slice(0, 5)
    .map((item) => model.projection!.state.tracks[item.track_id]);
  const frequent = [...available]
    .sort(
      (a, b) =>
        b.completed_plays - a.completed_plays ||
        b.last_played_ms - a.last_played_ms,
    )
    .slice(0, 5)
    .map((item) => model.projection!.state.tracks[item.track_id]);
  return (
    <section>
      <div className="section-heading">
        <h2>Недавно прослушанные</h2>
        <span className="muted small">На этом устройстве</span>
      </div>
      {error ? (
        <p role="alert">{error}</p>
      ) : recent.length ? (
        <TrackList {...props} tracks={recent} compact />
      ) : (
        <p className="section-empty">
          Здесь появятся композиции, которые вы дослушали. История хранится на
          этом устройстве.
        </p>
      )}
      {!!frequent.length && (
        <>
          <div className="section-heading">
            <h2>Часто слушаете</h2>
          </div>
          <TrackList {...props} tracks={frequent} compact />
        </>
      )}
    </section>
  );
}
