import { useEffect, useRef, useState } from "react";
import { Check, Save } from "lucide-react";
import { assertApplied, call, uuid } from "../../backend";
import type {
  ImportPreview,
  MediaEdit,
  MetadataPatch,
  Track,
} from "../../backend";
import type { Runtime } from "../../useBackend";
import { Loading, Modal } from "../../components/ui/Primitives";
import { acquireArtwork, artworkUrl } from "../../services/artwork";
import { readDraft, writeDraft } from "../../services/drafts";
import type { MetadataDraft } from "../../services/drafts";
import { ArtworkEditor } from "./ArtworkEditor";
import { LyricsEditor } from "../lyrics/LyricsEditor";
const textFields = [
  "title",
  "performer",
  "album",
  "album_artist",
  "genre",
  "composer",
  "comment",
  "copyright",
] as const;
const numericFields = ["year", "track_number", "disc_number"] as const;
export function MetadataEditor({
  runtime,
  track,
  preview,
  onClose,
  onImport,
}: {
  runtime: Runtime;
  track?: Track;
  preview?: ImportPreview;
  onClose: () => void;
  onImport?: (edit: MediaEdit, writeTags: boolean) => Promise<void>;
}) {
  const initial = track?.metadata ?? preview!.metadata;
  const context = runtime.model.status!.context;
  const key = `${context.profile_id}:${context.vault_id}:${track?.track_id ?? preview!.preview_id}`;
  const [draft, setDraft] = useState<MetadataDraft>(() => ({
    patch: {
      ...Object.fromEntries(textFields.map((f) => [f, initial[f] ?? ""])),
      ...Object.fromEntries(numericFields.map((f) => [f, initial[f] ?? 0])),
    },
    lyrics: preview?.lyrics ?? null,
    lyricsChanged: false,
    artwork: null,
    artworkChanged: false,
    order: track?.updated_order,
    savedAt: Date.now(),
    opId: uuid(),
  }));
  const [loaded, setLoaded] = useState(false);
  const [saving, setSaving] = useState(false);
  const [draftError, setDraftError] = useState("");
  const [section, setSection] = useState("main");
  const [writeTags, setWriteTags] = useState(false);
  const [originalCover, setOriginalCover] = useState<string>();
  const [persisted, setPersisted] = useState(false);
  const [lockedAttempt, setLockedAttempt] = useState(false);
  const current = useRef(draft);
  current.current = draft;
  const sending = useRef(false);
  const completed = useRef(false);
  useEffect(() => {
    let active = true;
    void readDraft(key)
      .then(async (saved) => {
        if (!active) return;
        if (saved) {
          setDraft(saved);
          setWriteTags(saved.attempt?.writeTags ?? saved.writeTags ?? false);
          setLockedAttempt(!!saved.attempt);
          setPersisted(true);
          return;
        }
        if (track?.metadata.lyrics) {
          const lyrics = await call("lyrics", {
            session_id: context.session_id,
            track_id: track.track_id,
          });
          if (active) setDraft((d) => ({ ...d, lyrics }));
        }
      })
      .catch(() => {
        if (active)
          setDraftError(
            "Не удалось восстановить черновик или текст. Несохранённые изменения останутся в этом окне.",
          );
      })
      .finally(() => {
        if (active) setLoaded(true);
      });
    return () => {
      active = false;
    };
  }, [key]);
  useEffect(() => {
    let active = true;
    let release: (() => void) | undefined;
    if (preview?.artwork) {
      const url = artworkUrl(preview.artwork);
      setOriginalCover(url);
      release = () => URL.revokeObjectURL(url);
    } else if (track?.metadata.artwork)
      void acquireArtwork(
        context.session_id,
        track.track_id,
        track.metadata.artwork.sha256,
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
  }, [
    context.session_id,
    track?.track_id,
    track?.metadata.artwork?.sha256,
    preview?.preview_id,
  ]);
  useEffect(() => {
    if (!loaded || completed.current) return;
    setPersisted(false);
    const timer = setTimeout(() => {
      if (completed.current) return;
      void writeDraft(key, draft)
        .then(() => setPersisted(true))
        .catch(() =>
          setDraftError(
            "Хранилище черновиков недоступно. Не закрывайте окно до сохранения.",
          ),
        );
    }, 350);
    return () => clearTimeout(timer);
  }, [key, draft, loaded]);
  const change = (patch: MetadataPatch) => {
    if (lockedAttempt) return;
    setDraft((d) => ({
      ...d,
      patch: { ...d.patch, ...patch },
      savedAt: Date.now(),
    }));
  };
  const latest =
    track && runtime.model.projection?.state.tracks[track.track_id];
  const conflict = !!track && (!latest || latest.updated_order !== draft.order);
  const online = runtime.model.status?.mode === "ONLINE_READY";
  const save = async () => {
    if (sending.current || !online || (conflict && !draft.attempt)) return;
    sending.current = true;
    setSaving(true);
    setDraftError("");
    const d = current.current;
    const edit: MediaEdit = d.attempt?.edit ?? {
      patch: {
        ...d.patch,
        ...(track ? { expected_order: d.order } : {}),
        clear_artwork: d.artworkChanged && !d.artwork,
        clear_lyrics: d.lyricsChanged && !d.lyrics,
      },
      artwork: d.artworkChanged ? d.artwork : null,
      lyrics: d.lyricsChanged ? d.lyrics : null,
    };
    const attempt = d.attempt ?? { edit, writeTags };
    const pending = { ...d, attempt, writeTags: attempt.writeTags };
    await runtime.run("Сохранение метаданных", async () => {
      await writeDraft(key, pending);
      current.current = pending;
      setDraft(pending);
      setLockedAttempt(true);
      if (track) {
        const reply = await call("vault", {
          session_id: context.session_id,
          request: {
            kind: attempt.writeTags ? "rewrite_track" : "edit_metadata",
            payload: { op_id: d.opId, track_id: track.track_id, edit },
          },
        });
        assertApplied(reply);
      } else {
        if (!onImport) throw new Error("Missing import callback");
        await onImport(edit, attempt.writeTags);
      }
      completed.current = true;
      await writeDraft(key);
      onClose();
    });
    sending.current = false;
    setSaving(false);
  };
  const close = () => {
    if (sending.current) return;
    if (loaded && !completed.current)
      void writeDraft(key, current.current).catch(() => {});
    onClose();
  };
  return (
    <Modal
      title={track ? "Изменить информацию" : "Редактирование перед отправкой"}
      wide
      onClose={close}
    >
      {!loaded ? (
        <Loading label="Восстанавливаем данные и черновик…" />
      ) : (
        <>
          <div
            className="editor-tabs"
            role="tablist"
            aria-label="Разделы редактора"
          >
            {[
              ["main", "Основное"],
              ["album", "Альбом"],
              ["artwork", "Обложка"],
              ["lyrics", "Текст песни"],
              ["more", "Дополнительно"],
            ].map(([id, label]) => (
              <button
                key={id}
                role="tab"
                aria-selected={section === id}
                onClick={() => setSection(id)}
              >
                {label}
              </button>
            ))}
          </div>
          {!online && (
            <p className="notice" role="status">
              Можно продолжать редактирование. Отправка станет доступна после
              восстановления соединения.
            </p>
          )}
          {conflict && (
            <div className="notice" role="alert">
              <strong>
                {latest
                  ? "Композиция изменилась на другом устройстве"
                  : "Композиция удалена из библиотеки"}
              </strong>
              <p>Ваш черновик сохранён. Сравните данные перед отправкой.</p>
              {latest && (
                <>
                  <details>
                    <summary>Текущая версия в Vault</summary>
                    <dl className="metadata-summary">
                      {textFields.map((f) => (
                        <div key={f}>
                          <dt>{f}</dt>
                          <dd>{latest.metadata[f] || "—"}</dd>
                        </div>
                      ))}
                    </dl>
                    <p>
                      Текст:{" "}
                      {latest.metadata.lyrics?.sha256 ===
                      track?.metadata.lyrics?.sha256
                        ? "без изменений"
                        : "изменён"}
                    </p>
                  </details>
                  <button
                    onClick={() =>
                      void runtime.run(
                        "Проверка версии",
                        async () => {
                          if (draft.attempt) {
                            const result = await call("query", {
                              session_id: context.session_id,
                              query: {
                                kind: "command_status",
                                payload: { op_id: draft.opId },
                              },
                            });
                            const command = result.result as {
                              status: string;
                            } | null;
                            if (
                              !command ||
                              ![
                                "REJECTED_BY_REDUCER",
                                "TELEGRAM_ERROR",
                              ].includes(command.status)
                            )
                              throw {
                                code: "CONFLICT",
                                message:
                                  "Сначала проверьте исход предыдущего сохранения. Новая операция пока заблокирована.",
                                retryable: false,
                              };
                          }
                          setDraft((d) => ({
                            ...d,
                            order: latest.updated_order,
                            opId: uuid(),
                            attempt: undefined,
                          }));
                          setLockedAttempt(false);
                        },
                        false,
                      )
                    }
                  >
                    Сохранить мой черновик поверх этой версии
                  </button>
                </>
              )}
            </div>
          )}
          {lockedAttempt && !saving && (
            <p className="notice">
              Исход предыдущего сохранения проверяется с тем же идентификатором.
              Повторная кнопка «Сохранить» не создаёт новую отправку.
            </p>
          )}
          <fieldset
            disabled={saving || lockedAttempt}
            className="editor-fields"
          >
            {section === "main" && (
              <div className="form-grid">
                <label className="span-two">
                  Название
                  <input
                    value={draft.patch.title ?? ""}
                    onChange={(e) => change({ title: e.target.value })}
                    maxLength={512}
                    required
                  />
                </label>
                <label className="span-two">
                  Исполнитель
                  <input
                    value={draft.patch.performer ?? ""}
                    onChange={(e) => change({ performer: e.target.value })}
                    maxLength={512}
                  />
                </label>
                <label>
                  Жанр
                  <input
                    value={draft.patch.genre ?? ""}
                    onChange={(e) => change({ genre: e.target.value })}
                    maxLength={256}
                  />
                </label>
                <label>
                  Год
                  <input
                    type="number"
                    min={0}
                    max={9999}
                    value={draft.patch.year || ""}
                    onChange={(e) => change({ year: Number(e.target.value) })}
                  />
                </label>
              </div>
            )}
            {section === "album" && (
              <div className="form-grid">
                <label className="span-two">
                  Альбом
                  <input
                    value={draft.patch.album ?? ""}
                    maxLength={512}
                    onChange={(e) => change({ album: e.target.value })}
                  />
                </label>
                <label className="span-two">
                  Исполнитель альбома
                  <input
                    value={draft.patch.album_artist ?? ""}
                    maxLength={512}
                    onChange={(e) => change({ album_artist: e.target.value })}
                  />
                </label>
                <label>
                  Номер трека
                  <input
                    type="number"
                    min={0}
                    max={9999}
                    value={draft.patch.track_number || ""}
                    onChange={(e) =>
                      change({ track_number: Number(e.target.value) })
                    }
                  />
                </label>
                <label>
                  Номер диска
                  <input
                    type="number"
                    min={0}
                    max={999}
                    value={draft.patch.disc_number || ""}
                    onChange={(e) =>
                      change({ disc_number: Number(e.target.value) })
                    }
                  />
                </label>
              </div>
            )}
            {section === "artwork" && (
              <ArtworkEditor
                bytes={draft.artwork}
                initialUrl={draft.artworkChanged ? undefined : originalCover}
                disabled={saving || lockedAttempt}
                onChange={(artwork) =>
                  setDraft((d) => ({ ...d, artwork, artworkChanged: true }))
                }
              />
            )}
            {section === "lyrics" && (
              <LyricsEditor
                value={draft.lyrics}
                disabled={saving || lockedAttempt}
                position={
                  runtime.model.playback.track_id === track?.track_id
                    ? runtime.model.playback.position_ms
                    : undefined
                }
                onChange={(lyrics) =>
                  setDraft((d) => ({ ...d, lyrics, lyricsChanged: true }))
                }
              />
            )}
            {section === "more" && (
              <div className="form-grid">
                <label className="span-two">
                  Композитор
                  <input
                    maxLength={512}
                    value={draft.patch.composer ?? ""}
                    onChange={(e) => change({ composer: e.target.value })}
                  />
                </label>
                <label className="span-two">
                  Copyright
                  <input
                    maxLength={512}
                    value={draft.patch.copyright ?? ""}
                    onChange={(e) => change({ copyright: e.target.value })}
                  />
                </label>
                <label className="span-two">
                  Комментарий
                  <textarea
                    rows={4}
                    maxLength={1024}
                    value={draft.patch.comment ?? ""}
                    onChange={(e) => change({ comment: e.target.value })}
                  />
                </label>
              </div>
            )}
          </fieldset>
          {
            <div className="save-mode">
              <label>
                <input
                  type="radio"
                  name="save-mode"
                  checked={!writeTags}
                  disabled={saving || lockedAttempt}
                  onChange={() => {
                    setWriteTags(false);
                    setDraft((d) => ({ ...d, writeTags: false }));
                  }}
                />
                <span>
                  <strong>Изменить метаданные Kiyora</strong>
                  <small>Аудиобайты останутся оригинальными.</small>
                </span>
              </label>
              <label>
                <input
                  type="radio"
                  name="save-mode"
                  checked={writeTags}
                  disabled={saving || lockedAttempt}
                  onChange={() => {
                    setWriteTags(true);
                    setDraft((d) => ({ ...d, writeTags: true }));
                  }}
                />
                <span>
                  <strong>Изменить встроенные теги копии</strong>
                  <small>
                    Будет загружена отдельная копия с тегами. Оригинал
                    сохранится.
                  </small>
                </span>
              </label>
            </div>
          }
          {track && (
            <p className="muted small">
              {writeTags
                ? "Будет создан новый источник аудио. Идентификатор композиции, избранное и плейлисты сохранятся. Предыдущий источник не удаляется."
                : "Изменения сохраняются в каталоге Kiyora. Аудиофайл не скачивается и не перезаписывается."}
            </p>
          )}
          {draftError && <p role="alert">{draftError}</p>}
          <div className="editor-footer">
            <span className="muted small">
              {persisted ? (
                <>
                  <Check size={14} />
                  Черновик на этом устройстве
                </>
              ) : (
                "Сохраняем черновик…"
              )}
            </span>
            <div className="actions">
              <button onClick={close}>Закрыть</button>
              <button
                className="primary"
                onClick={() => void save()}
                disabled={
                  saving ||
                  !online ||
                  (conflict && !draft.attempt) ||
                  !draft.patch.title?.trim()
                }
              >
                <Save />
                {saving
                  ? "Сохраняем…"
                  : lockedAttempt
                    ? "Проверить сохранение"
                    : track
                      ? "Сохранить"
                      : "Сохранить и отправить"}
              </button>
            </div>
          </div>
        </>
      )}
    </Modal>
  );
}
