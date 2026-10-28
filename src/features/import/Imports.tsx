import { useEffect, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { Check, FileAudio, Pencil, Upload, X } from "lucide-react";
import {
  assertApplied,
  bytesLabel,
  call,
  duration,
  errorOf,
  resumeJob,
  uuid,
} from "../../backend";
import type { ImportPreview, MediaEdit } from "../../backend";
import type { Runtime } from "../../useBackend";
import {
  Empty,
  IconButton,
  Loading,
  PageHeading,
} from "../../components/ui/Primitives";
import { artworkUrl } from "../../services/artwork";
import { MetadataEditor } from "../metadata-editor/MetadataEditor";
const stages: Record<string, string> = {
  staging: "Подготовка копии",
  prepared: "Готов к отправке",
  upload_unknown: "Проверяем исход отправки",
  uploaded: "Подтверждаем регистрацию",
  complete: "Добавлено в библиотеку",
};
function PreviewImage({ preview }: { preview: ImportPreview }) {
  const [url, setUrl] = useState<string>();
  useEffect(() => {
    if (!preview.artwork) return;
    const url = artworkUrl(preview.artwork);
    setUrl(url);
    return () => URL.revokeObjectURL(url);
  }, [preview.preview_id]);
  return (
    <div className="import-cover">
      {url ? (
        <img src={url} alt="Обложка выбранного файла" />
      ) : (
        <FileAudio size={40} strokeWidth={1} />
      )}
    </div>
  );
}
export function ImportsScreen({ runtime }: { runtime: Runtime }) {
  const { model, run } = runtime;
  const session_id = model.status!.context.session_id;
  const online = model.status?.mode === "ONLINE_READY";
  const [previews, setPreviews] = useState<ImportPreview[]>([]);
  const [editing, setEditing] = useState<ImportPreview | null>(null);
  const [preparing, setPreparing] = useState(false);
  const [dragging, setDragging] = useState(false);
  const [error, setError] = useState("");
  const [allowDuplicate, setAllowDuplicate] = useState(false);
  const [starting, setStarting] = useState<Set<string>>(new Set());
  const active = useRef(true);
  const lock = useRef(false);
  useEffect(() => {
    active.current = true;
    void call("vault", {
      session_id,
      request: { kind: "list_import_previews" },
    })
      .then((r) => {
        if (active.current) setPreviews(r.result as ImportPreview[]);
      })
      .catch((e) => {
        if (active.current) setError(errorOf(e).message);
      });
    return () => {
      active.current = false;
    };
  }, [session_id]);
  const prepare = async (paths: string[]) => {
    if (lock.current) return;
    lock.current = true;
    setPreparing(true);
    setError("");
    try {
      for (const path of paths.slice(0, 50)) {
        const preview_id = uuid();
        const reply = await call("vault", {
          session_id,
          request: { kind: "preview_import", payload: { path, preview_id } },
        });
        if (active.current && reply.kind === "import_preview")
          setPreviews((p) => [...p, reply.result as ImportPreview]);
      }
    } catch (e) {
      if (active.current) setError(errorOf(e).message);
    } finally {
      lock.current = false;
      if (active.current) setPreparing(false);
    }
  };
  const prepareRef = useRef(prepare);
  prepareRef.current = prepare;
  useEffect(() => {
    let disposed = false;
    let off: (() => void) | undefined;
    void getCurrentWebview()
      .onDragDropEvent((e) => {
        if (disposed) return;
        setDragging(e.payload.type === "enter" || e.payload.type === "over");
        if (e.payload.type === "drop") {
          setDragging(false);
          void prepareRef.current(e.payload.paths);
        }
      })
      .then((fn) => {
        if (disposed) fn();
        else off = fn;
      })
      .catch(() => {});
    return () => {
      disposed = true;
      off?.();
    };
  }, []);
  const commit = async (
    preview: ImportPreview,
    edit: MediaEdit | null,
    write_tags: boolean,
    background: boolean,
  ) => {
    const payload = {
      preview_id: preview.preview_id,
      track_id: preview.preview_id,
      source_id: preview.preview_id,
      edit,
      write_tags,
      allow_duplicate: allowDuplicate,
    };
    if (background) {
      await call("start_job", {
        session_id,
        job_id: preview.preview_id,
        request: {
          kind: "vault",
          payload: { kind: "import_prepared", payload },
        },
      });
    } else {
      const reply = await call("vault", {
        session_id,
        request: { kind: "import_prepared", payload },
      });
      if (reply.kind === "import_review")
        throw {
          code: "CONFLICT",
          message:
            "Найден дубликат. Закройте редактор и разрешите отдельную копию, если это намеренно.",
          retryable: false,
        };
      assertApplied(reply);
      setPreviews((p) => p.filter((v) => v.preview_id !== preview.preview_id));
    }
  };
  return (
    <>
      <PageHeading
        title="Добавить музыку"
        subtitle="Сначала проверьте файлы. Затем решите, что отправить."
      />
      <div className={`import-drop ${dragging ? "dragging" : ""}`}>
        <Upload size={28} />
        <div>
          <h2>Ваша следующая любимая композиция</h2>
          <p>
            MP3, FLAC, M4A, AAC, OGG, Opus, WAV и AIFF. На компьютере файлы
            можно перетащить сюда.
          </p>
        </div>
        <button
          className="primary"
          disabled={preparing}
          onClick={() =>
            void run(
              "Выбор файлов",
              async () => {
                const paths = await open({
                  multiple: true,
                  directory: false,
                  filters: [
                    {
                      name: "Аудио",
                      extensions: [
                        "mp3",
                        "flac",
                        "m4a",
                        "mp4",
                        "aac",
                        "ogg",
                        "opus",
                        "wav",
                        "aiff",
                        "aif",
                      ],
                    },
                  ],
                });
                if (paths)
                  await prepare(Array.isArray(paths) ? paths : [paths]);
              },
              false,
            )
          }
        >
          <Upload />
          Выбрать файлы
        </button>
      </div>
      {preparing && (
        <Loading label="Читаем метаданные и готовим локальные копии…" />
      )}
      {error && (
        <p className="inline-error" role="alert">
          {error}
        </p>
      )}
      {!!previews.length && (
        <label className="check import-duplicates">
          <input
            type="checkbox"
            checked={allowDuplicate}
            onChange={(e) => setAllowDuplicate(e.target.checked)}
          />
          Разрешить отдельную копию уже существующей композиции
        </label>
      )}
      <div className="import-previews">
        {previews.map((preview) => {
          const job = model.jobs.find((j) => j.job_id === preview.preview_id);
          const finished =
            job?.state === "succeeded" &&
            job.result?.result.kind === "command" &&
            (job.result.result.result as { status?: string })?.status ===
              "APPLIED";
          const duplicate = job?.result?.result.kind === "import_review";
          return (
            <section className="import-preview" key={preview.preview_id}>
              <PreviewImage preview={preview} />
              <div className="import-details">
                <div className="section-heading">
                  <div>
                    <h2>{preview.metadata.title}</h2>
                    <p>{preview.filename}</p>
                  </div>
                  {!job && (
                    <IconButton
                      label="Убрать выбранный файл"
                      disabled={starting.has(preview.preview_id)}
                      onClick={() =>
                        void run(
                          "Удаление предварительной копии",
                          async () => {
                            await call("vault", {
                              session_id,
                              request: {
                                kind: "discard_import_preview",
                                payload: { preview_id: preview.preview_id },
                              },
                            });
                            setPreviews((p) =>
                              p.filter(
                                (v) => v.preview_id !== preview.preview_id,
                              ),
                            );
                          },
                          false,
                        )
                      }
                    >
                      <X />
                    </IconButton>
                  )}
                </div>
                <dl className="metadata-summary">
                  <div>
                    <dt>Исполнитель</dt>
                    <dd>{preview.metadata.performer || "Не указан"}</dd>
                  </div>
                  <div>
                    <dt>Альбом</dt>
                    <dd>{preview.metadata.album || "Не указан"}</dd>
                  </div>
                  <div>
                    <dt>Длительность</dt>
                    <dd>{duration(preview.metadata.duration_ms)}</dd>
                  </div>
                  <div>
                    <dt>Файл</dt>
                    <dd>
                      {preview.metadata.codec.toUpperCase()} ·{" "}
                      {bytesLabel(preview.size_bytes)}
                    </dd>
                  </div>
                  <div>
                    <dt>Год / жанр</dt>
                    <dd>
                      {preview.metadata.year || "—"} /{" "}
                      {preview.metadata.genre || "—"}
                    </dd>
                  </div>
                  <div>
                    <dt>Текст песни</dt>
                    <dd>
                      {preview.lyrics
                        ? `${preview.lyrics.lines.length} строк`
                        : "Нет"}
                    </dd>
                  </div>
                </dl>
                {preview.warnings.map((w) => (
                  <p className="notice" key={w}>
                    {w}
                  </p>
                ))}
                {finished ? (
                  <p className="success-text">
                    <Check />
                    Добавлено в библиотеку
                  </p>
                ) : job && !duplicate ? (
                  <p role="status">
                    {job.state === "running"
                      ? "Отправка и регистрация…"
                      : "Операция требует проверки. Возобновите её в журнале ниже."}
                  </p>
                ) : (
                  <>
                    <div className="actions">
                      <button
                        className="primary"
                        disabled={!online || starting.has(preview.preview_id)}
                        onClick={() => {
                          if (starting.has(preview.preview_id)) return;
                          setStarting((s) =>
                            new Set(s).add(preview.preview_id),
                          );
                          void run("Отправить оригинал", () =>
                            commit(preview, null, false, !duplicate),
                          ).finally(() =>
                            setStarting((s) => {
                              const next = new Set(s);
                              next.delete(preview.preview_id);
                              return next;
                            }),
                          );
                        }}
                      >
                        <Upload />
                        Отправить как есть
                      </button>
                      <button
                        disabled={starting.has(preview.preview_id) || !!job}
                        onClick={() => setEditing(preview)}
                      >
                        <Pencil />
                        Отредактировать данные
                      </button>
                    </div>
                    <p className="small muted">
                      {duplicate
                        ? "Найден дубликат. Если нужна отдельная копия, разрешите дубликаты выше и повторите отправку."
                        : "Ничего не отправляется в Telegram до вашего выбора."}
                    </p>
                  </>
                )}
              </div>
            </section>
          );
        })}
      </div>
      {!previews.length && !preparing && (
        <Empty title="Готовы к новой музыке">
          Выберите файлы. Kiyora покажет обложки и метаданные перед отправкой.
        </Empty>
      )}
      <section>
        <h2>Журнал операций</h2>
        {!model.jobs.length && !model.imports.length ? (
          <p className="section-empty">
            Здесь появятся реальные этапы импорта.
          </p>
        ) : (
          <div className="job-list">
            {model.jobs.map((job) => (
              <div className="job-row" key={job.job_id}>
                <div>
                  <strong>
                    {job.state === "running"
                      ? "Выполняется"
                      : job.state === "succeeded"
                        ? "Операция завершена"
                        : job.state === "cancelled"
                          ? "Отменено"
                          : job.state === "interrupted"
                            ? "Прервано"
                            : "Ошибка"}
                  </strong>
                  <p>
                    {job.completed} / {job.total} операций
                  </p>
                  {job.error && (
                    <p className="error-text">{job.error.message}</p>
                  )}
                  {job.result?.result.kind === "import_review" && (
                    <p>Найден дубликат. Повторная загрузка не выполнена.</p>
                  )}
                </div>
                {job.state === "running" ? (
                  <button
                    onClick={() =>
                      void run("Отмена", () =>
                        call("cancel_job", { session_id, job_id: job.job_id }),
                      )
                    }
                  >
                    Отменить
                  </button>
                ) : (
                  job.state !== "succeeded" && (
                    <button
                      disabled={!online}
                      onClick={() =>
                        void run("Восстановление операции", () =>
                          resumeJob(session_id, job.job_id),
                        )
                      }
                    >
                      Возобновить
                    </button>
                  )
                )}
              </div>
            ))}
            {model.imports.map((item) => (
              <div className="job-row" key={item.op_id}>
                <div>
                  <strong>{item.metadata.title}</strong>
                  <p>{stages[item.stage]}</p>
                </div>
                {item.stage !== "complete" && (
                  <button
                    disabled={
                      !online || model.jobs.some((j) => j.state === "running")
                    }
                    onClick={() =>
                      void run("Восстановление импорта", () =>
                        call("start_job", {
                          session_id,
                          job_id: uuid(),
                          request: {
                            kind: "vault",
                            payload: {
                              kind: "resume_import",
                              payload: { op_id: item.op_id },
                            },
                          },
                        }),
                      )
                    }
                  >
                    Проверить и продолжить
                  </button>
                )}
              </div>
            ))}
          </div>
        )}
      </section>
      {editing && (
        <MetadataEditor
          runtime={runtime}
          preview={editing}
          onClose={() => setEditing(null)}
          onImport={(edit, tags) => commit(editing, edit, tags, false)}
        />
      )}
    </>
  );
}
