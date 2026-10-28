import { useEffect, useRef, useState } from "react";
import { assertApplied, call, errorOf, uuid } from "../../backend";
import type {
  Playlist,
  PlaylistEdit,
  VaultRequest,
  Command,
} from "../../backend";
import type { Runtime } from "../../useBackend";
import { Loading, Modal } from "../../components/ui/Primitives";
import { ArtworkEditor } from "../metadata-editor/ArtworkEditor";
import { acquireArtwork } from "../../services/artwork";
import { readDraft, writeDraft } from "../../services/drafts";

type Draft = {
  id: string;
  op: string;
  title: string;
  order?: string;
  artwork: number[] | null;
  artworkChanged: boolean;
  attempt?: VaultRequest;
};
export function PlaylistForm({
  runtime,
  playlist,
  onClose,
  onSaved,
}: {
  runtime: Runtime;
  playlist?: Playlist;
  onClose: () => void;
  onSaved: (id: string) => void;
}) {
  const context = runtime.model.status!.context;
  const key = `playlist:${context.profile_id}:${context.vault_id}:${playlist?.playlist_id ?? "new"}`;
  const [draft, setDraft] = useState<Draft>(() => ({
    id: playlist?.playlist_id ?? uuid(),
    op: uuid(),
    title: playlist?.title ?? "",
    order: playlist?.updated_order,
    artwork: null,
    artworkChanged: false,
  }));
  const [loaded, setLoaded] = useState(false);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");
  const [originalCover, setOriginalCover] = useState<string>();
  const current = useRef(draft);
  current.current = draft;
  const sending = useRef(false);
  const mounted = useRef(false);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);
  const completed = useRef(false);
  useEffect(() => {
    let active = true;
    void readDraft<Draft>(key)
      .then((value) => {
        if (active) {
          if (value) setDraft(value);
          setLoaded(true);
        }
      })
      .catch(() => {
        if (active)
          setError(
            "Не удалось прочитать черновик. Закройте окно и повторите открытие.",
          );
      });
    return () => {
      active = false;
    };
  }, [key]);
  useEffect(() => {
    let active = true;
    let release: (() => void) | undefined;
    if (playlist?.artwork)
      void acquireArtwork(
        context.session_id,
        playlist.playlist_id,
        playlist.artwork.sha256,
        "playlist",
      )
        .then((asset) => {
          if (!active) asset?.release();
          else if (asset) {
            release = asset.release;
            setOriginalCover(asset.url);
          }
        })
        .catch(() => {});
    return () => {
      active = false;
      release?.();
    };
  }, [context.session_id, playlist?.playlist_id, playlist?.artwork?.sha256]);
  useEffect(() => {
    if (!loaded || completed.current) return;
    let active = true;
    const timer = setTimeout(() => {
      if (!completed.current)
        void writeDraft(key, draft).catch(() => {
          if (active)
            setError(
              "Не удалось сохранить черновик на устройстве. Не закрывайте окно.",
            );
        });
    }, 300);
    return () => {
      active = false;
      clearTimeout(timer);
    };
  }, [key, loaded, draft]);
  const latest =
    playlist && runtime.model.projection?.state.playlists[playlist.playlist_id];
  const conflict =
    !!playlist && (!latest || latest.updated_order !== draft.order);
  const online = runtime.model.status?.mode === "ONLINE_READY";
  const locked = saving || !!draft.attempt;
  const change = (patch: Partial<Draft>) => {
    if (!locked) setDraft((value) => ({ ...value, ...patch }));
  };
  const close = () => {
    if (sending.current) return;
    if (!loaded || completed.current) {
      onClose();
      return;
    }
    void writeDraft(key, current.current)
      .then(onClose)
      .catch(() =>
        setError("Не удалось сохранить черновик. Окно оставлено открытым."),
      );
  };
  const save = async () => {
    if (sending.current || !loaded || !online || (conflict && !draft.attempt))
      return;
    sending.current = true;
    setSaving(true);
    setError("");
    const d = current.current;
    const edit: PlaylistEdit | undefined = playlist
      ? {
          title: d.title.trim(),
          expected_order: d.order!,
          artwork: d.artworkChanged ? d.artwork : null,
          clear_artwork: d.artworkChanged && !d.artwork,
        }
      : undefined;
    const attempt: VaultRequest =
      d.attempt ??
      (edit
        ? {
            kind: "edit_playlist",
            payload: { op_id: d.op, playlist_id: d.id, edit },
          }
        : {
            kind: "mutate",
            payload: {
              op_id: d.op,
              entity_id: d.id,
              action: {
                action: "playlist.create",
                payload: { title: d.title.trim() },
              },
            },
          });
    const pending = { ...d, attempt };
    try {
      await writeDraft(key, pending);
      current.current = pending;
      setDraft(pending);
      const reply = await call("vault", {
        session_id: context.session_id,
        request: attempt,
      });
      if (
        reply.kind === "command" &&
        (reply.result as Command).status === "REJECTED_BY_REDUCER"
      ) {
        throw {
          code: "EDIT_NOT_SENT",
          message:
            "Изменение отклонено. Проверьте актуальную версию плейлиста.",
        };
      }
      assertApplied(reply);
      completed.current = true;
      await writeDraft(key);
      await runtime.refresh();
      if (mounted.current) onSaved(d.id);
    } catch (e) {
      const failure = errorOf(e);
      if (failure.code === "EDIT_NOT_SENT") {
        const revised = { ...pending, attempt: undefined, op: uuid() };
        current.current = revised;
        setDraft(revised);
        try {
          await writeDraft(key, revised);
        } catch {
          /* The exact old intent remains recoverable. */
        }
        setError(
          "Изменение не применено. Проверьте текущую версию и сохраните черновик снова.",
        );
      } else setError(failure.message);
    } finally {
      sending.current = false;
      setSaving(false);
    }
  };
  return (
    <Modal
      title={playlist ? "Изменить плейлист" : "Новый плейлист"}
      wide={!!playlist}
      onClose={close}
    >
      {!loaded ? (
        <>
          {error ? (
            <p role="alert">{error}</p>
          ) : (
            <Loading label="Открываем черновик…" />
          )}
        </>
      ) : (
        <div className="stack playlist-editor">
          {!online && (
            <p className="notice" role="status">
              Черновик остаётся на устройстве. Сохранение в Vault доступно после
              восстановления соединения.
            </p>
          )}
          {draft.attempt && (
            <p className="notice" role="status">
              Проверяем предыдущее сохранение. Данные заблокированы до
              подтверждения результата.
            </p>
          )}
          {conflict && !draft.attempt && (
            <div className="notice" role="alert">
              <p>
                {latest
                  ? "Плейлист изменён на другом устройстве. Сравните его с черновиком."
                  : "Плейлист удалён на другом устройстве."}
              </p>
              {latest && (
                <>
                  <p>
                    Текущее название: <strong>{latest.title}</strong>
                  </p>
                  <p>
                    При сохранении будут применены название и выбранная в
                    черновике обложка.
                  </p>
                  <button
                    onClick={() =>
                      change({ order: latest.updated_order, op: uuid() })
                    }
                  >
                    Применить черновик к текущей версии
                  </button>
                </>
              )}
            </div>
          )}
          <label>
            Название
            <input
              autoFocus
              value={draft.title}
              maxLength={512}
              disabled={locked}
              onChange={(e) => change({ title: e.target.value })}
              placeholder="Музыка для…"
            />
          </label>
          {playlist && (
            <ArtworkEditor
              bytes={draft.artwork}
              initialUrl={draft.artworkChanged ? undefined : originalCover}
              disabled={locked}
              onChange={(artwork) => change({ artwork, artworkChanged: true })}
            />
          )}
          {playlist && (
            <p className="muted">
              Обложка сохраняется в Vault. Без собственной обложки используется
              мозаика из композиций.
            </p>
          )}
          {error && <p role="alert">{error}</p>}
          <div className="dialog-actions">
            <button type="button" onClick={close} disabled={saving}>
              Закрыть
            </button>
            <button
              className="primary"
              disabled={
                saving ||
                !online ||
                !draft.title.trim() ||
                (conflict && !draft.attempt)
              }
              onClick={() => void save()}
            >
              {saving
                ? "Сохраняем…"
                : draft.attempt
                  ? "Проверить сохранение"
                  : "Сохранить"}
            </button>
          </div>
        </div>
      )}
    </Modal>
  );
}
