import { useState } from "react";
import { call, duration, errorOf } from "../../backend";
import type { LyricsDocument } from "../../backend";
export function LyricsEditor({
  value,
  onChange,
  position,
  disabled,
}: {
  value: LyricsDocument | null;
  onChange: (value: LyricsDocument | null) => void;
  position?: number;
  disabled: boolean;
}) {
  const [raw, setRaw] = useState("");
  const [format, setFormat] = useState("plain");
  const [error, setError] = useState("");
  const [shift, setShift] = useState("0");
  const [busy, setBusy] = useState(false);
  const [editRows, setEditRows] = useState(false);
  const parse = async (text: string, type: string) => {
    setBusy(true);
    setError("");
    try {
      const parsed = await call("parse_lyrics", { text, format: type });
      onChange(parsed);
      setEditRows(true);
    } catch (e) {
      setError(errorOf(e).message + " Проверьте временные метки и их порядок.");
    } finally {
      setBusy(false);
    }
  };
  const synced = value?.lines.some((l) => l.start_ms !== null);
  return (
    <div className="lyrics-editor">
      <div className="section-heading">
        <h3>Текст и тайминги</h3>
        <label className="file-button">
          Импорт текста
          <input
            type="file"
            accept=".txt,.lrc,.srt,.vtt,text/plain"
            disabled={disabled || busy}
            onChange={(e) => {
              const file = e.target.files?.[0];
              e.target.value = "";
              if (!file) return;
              if (file.size > 128 * 1024) {
                setError("Текст должен быть не больше 128 КБ.");
                return;
              }
              void file
                .text()
                .then((text) =>
                  parse(
                    text,
                    file.name.split(".").pop()?.toLowerCase() ?? "plain",
                  ),
                )
                .catch(() => setError("Не удалось прочитать файл."));
            }}
          />
        </label>
      </div>
      {value?.lines.length ? (
        <div className="actions">
          <span>
            {value.lines.length} строк ·{" "}
            {synced ? "С таймингами" : "Обычный текст"}
          </span>
          <button type="button" onClick={() => setEditRows((v) => !v)}>
            {editRows ? "Скрыть строки" : "Редактировать строки"}
          </button>
          <button
            type="button"
            disabled={disabled}
            onClick={() => {
              onChange(null);
              setEditRows(false);
            }}
          >
            Удалить текст
          </button>
        </div>
      ) : (
        <p className="muted">
          Вставьте текст или выберите TXT, LRC, SRT, WebVTT. Текст будет
          доступен на других устройствах без скачивания аудио.
        </p>
      )}
      <details open={!value?.lines.length}>
        <summary>Вставить или заменить текст</summary>
        <label>
          Формат
          <select value={format} onChange={(e) => setFormat(e.target.value)}>
            <option value="plain">Обычный текст</option>
            <option value="lrc">LRC</option>
            <option value="srt">SRT</option>
            <option value="vtt">WebVTT</option>
          </select>
        </label>
        <label>
          <span className="sr-only">Содержимое текста</span>
          <textarea
            rows={8}
            value={raw}
            maxLength={131072}
            onChange={(e) => setRaw(e.target.value)}
            placeholder="Вставьте текст песни…"
            disabled={disabled}
          />
        </label>
        <button
          type="button"
          disabled={disabled || busy || !raw.trim()}
          onClick={() => void parse(raw, format)}
        >
          {busy ? "Проверяем…" : "Разобрать текст"}
        </button>
      </details>
      {error && (
        <p className="inline-error" role="alert">
          {error}
        </p>
      )}
      {value && editRows && (
        <>
          <div className="inline-form">
            <label>
              Сдвиг, мс
              <input
                type="number"
                value={shift}
                onChange={(e) => setShift(e.target.value)}
                min={-86400000}
                max={86400000}
              />
            </label>
            <button
              type="button"
              disabled={disabled || !synced || !Number.isFinite(Number(shift))}
              onClick={() => {
                const delta = Number(shift);
                onChange({
                  ...value,
                  lines: value.lines.map((l) => ({
                    ...l,
                    start_ms:
                      l.start_ms === null
                        ? null
                        : Math.max(0, l.start_ms + delta),
                    end_ms:
                      l.end_ms === null ? null : Math.max(0, l.end_ms + delta),
                  })),
                });
                setShift("0");
              }}
            >
              Сдвинуть все строки
            </button>
          </div>
          <div className="lyric-edit-lines">
            {value.lines.map((line, index) => (
              <div key={index}>
                <label>
                  <span className="sr-only">Начало строки {index + 1}, мс</span>
                  <input
                    type="number"
                    min={0}
                    max={604800000}
                    placeholder="Без времени"
                    aria-label={`Начало строки ${index + 1}, мс`}
                    value={line.start_ms ?? ""}
                    disabled={disabled}
                    onChange={(e) =>
                      onChange({
                        ...value,
                        lines: value.lines.map((l, i) =>
                          i === index
                            ? {
                                ...l,
                                start_ms:
                                  e.target.value === ""
                                    ? null
                                    : Number(e.target.value),
                                end_ms: null,
                              }
                            : l,
                        ),
                      })
                    }
                  />
                </label>
                <input
                  aria-label={`Текст строки ${index + 1}`}
                  value={line.text}
                  maxLength={4096}
                  disabled={disabled}
                  onChange={(e) =>
                    onChange({
                      ...value,
                      lines: value.lines.map((l, i) =>
                        i === index ? { ...l, text: e.target.value } : l,
                      ),
                    })
                  }
                />
                {position !== undefined && (
                  <button
                    type="button"
                    disabled={disabled}
                    title="Использовать текущую позицию плеера"
                    onClick={() =>
                      onChange({
                        ...value,
                        lines: value.lines.map((l, i) =>
                          i === index
                            ? { ...l, start_ms: position, end_ms: null }
                            : l,
                        ),
                      })
                    }
                  >
                    {duration(position)}
                  </button>
                )}
              </div>
            ))}
          </div>
          <p className="small muted">
            Для синхронизированного текста укажите время каждой строки по
            возрастанию. Время задаётся в миллисекундах.
          </p>
        </>
      )}
    </div>
  );
}
