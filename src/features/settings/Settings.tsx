import { useEffect, useState } from "react";
import { open, save } from "@tauri-apps/plugin-dialog";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
  Check,
  Copy,
  ExternalLink,
  HardDrive,
  Monitor,
  Moon,
  Sun,
} from "lucide-react";
import type { Runtime } from "../../useBackend";
import { bytesLabel, call, errorOf } from "../../backend";
import type { StorageUsage } from "../../backend";
import { Profiles, Authorization, VaultSetup } from "../../Setup";
import { Confirm, PageHeading } from "../../components/ui/Primitives";
import type { usePreferences } from "../../hooks/usePreferences";
import packageInfo from "../../../package.json";
export function SettingsScreen({
  runtime,
  preferences,
}: {
  runtime: Runtime;
  preferences: ReturnType<typeof usePreferences>;
}) {
  const { model, run } = runtime;
  const session_id = model.status!.context.session_id;
  const online = model.status?.mode === "ONLINE_READY";
  const [storage, setStorage] = useState<StorageUsage | null>(null);
  const [storageError, setStorageError] = useState("");
  const [quota, setQuota] = useState(10);
  const [copy, setCopy] = useState(false);
  const [logout, setLogout] = useState(false);
  const [restoring, setRestoring] = useState<string | null>(null);
  useEffect(() => {
    let active = true;
    void call("storage_usage", { session_id })
      .then((value) => {
        if (active) {
          setStorage(value);
          setQuota(Math.round(value.verified_cache.quota_bytes / 1073741824));
        }
      })
      .catch((e) => {
        if (active) setStorageError(errorOf(e).message);
      });
    return () => {
      active = false;
    };
  }, [session_id, model.revision]);
  const report = {
    version: packageInfo.version,
    runtime: model.status?.mode,
    telegram: model.info?.telegram_connection,
    authorization: model.auth?.state,
    vault_bound: !!model.status?.context.vault_id,
    projection_loaded: !!model.projection,
    repair_required: model.info?.repair_required,
    service_running: model.info?.service_running,
    tracks: Object.keys(model.projection?.state.tracks ?? {}).length,
    playlists: Object.keys(model.projection?.state.playlists ?? {}).length,
    playback: { state: model.playback.state, error: model.playback.code },
    jobs: model.jobs.map((j) => ({
      state: j.state,
      error: j.error?.code,
      completed: j.completed,
      total: j.total,
    })),
    imports: model.imports.map((i) => ({ stage: i.stage })),
    last_error: model.error?.code,
  };
  return (
    <>
      <PageHeading title="Настройки" subtitle="Kiyora в вашем ритме" />
      <section className="settings-section">
        <div>
          <h2>Внешний вид</h2>
          <p>Тема и движение интерфейса.</p>
        </div>
        <div className="settings-panel">
          <div className="theme-options">
            {(
              [
                { id: "dark", label: "Тёмная", Icon: Moon },
                { id: "light", label: "Светлая", Icon: Sun },
                { id: "system", label: "Системная", Icon: Monitor },
              ] as const
            ).map(({ id, label, Icon }) => (
              <button
                key={id}
                aria-pressed={preferences.theme === id}
                onClick={() => preferences.setTheme(id)}
              >
                <Icon />
                {label}
              </button>
            ))}
          </div>
          <label className="setting-toggle">
            <span>
              Уменьшить анимации
              <small>
                Системная настройка уменьшения движения тоже учитывается.
              </small>
            </span>
            <input
              type="checkbox"
              checked={preferences.reducedMotion}
              onChange={(e) => preferences.setReducedMotion(e.target.checked)}
            />
          </label>
        </div>
      </section>
      <section className="settings-section">
        <div>
          <h2>Загрузки и кэш</h2>
          <p>
            Закреплённая музыка сохраняется для офлайна. Активный плеер защищает
            используемые файлы.
          </p>
        </div>
        <div className="settings-panel">
          {storage ? (
            <>
              <div className="storage-number">
                <HardDrive />
                <strong>{bytesLabel(storage.verified_cache.used_bytes)}</strong>
                <span>кэш Kiyora</span>
              </div>
              <dl className="metadata-summary">
                <div>
                  <dt>Закреплено</dt>
                  <dd>{storage.verified_cache.pinned_entries} файлов</dd>
                </div>
                <div>
                  <dt>Данные Telegram</dt>
                  <dd>{bytesLabel(storage.tdlib_files_bytes)}</dd>
                </div>
                <div>
                  <dt>Подготовленные файлы</dt>
                  <dd>{bytesLabel(storage.staged_import_bytes)}</dd>
                </div>
              </dl>
            </>
          ) : (
            <p>{storageError || "Читаем использование диска…"}</p>
          )}
          <form
            className="inline-form"
            onSubmit={(e) => {
              e.preventDefault();
              void run("Лимит кэша", () =>
                call("vault", {
                  session_id,
                  request: {
                    kind: "set_cache_quota",
                    payload: { bytes: Math.round(quota * 1073741824) },
                  },
                }),
              );
            }}
          >
            <label>
              Лимит кэша, ГБ
              <input
                type="number"
                min={1}
                max={1024}
                value={quota}
                onChange={(e) => setQuota(Number(e.target.value))}
              />
            </label>
            <button
              disabled={!Number.isFinite(quota) || quota < 1 || quota > 1024}
            >
              Применить
            </button>
          </form>
          <button
            onClick={() =>
              void run("Очистка неиспользуемого кэша", () =>
                call("vault", {
                  session_id,
                  request: { kind: "clear_local_cache" },
                }),
              )
            }
          >
            Очистить незакреплённый кэш
          </button>
        </div>
      </section>
      <details className="settings-panel">
        <summary>Управление с клавиатуры</summary>
        <p>
          Пробел — пауза и воспроизведение. Ctrl / ⌘ + ← / → — предыдущий и
          следующий трек. Ctrl / ⌘ + K — поиск, Ctrl / ⌘ + L — медиатека. Alt +
          ← — назад. Escape — закрыть открытое представление.
        </p>
        <p className="muted">При вводе текста сочетания плеера отключены.</p>
      </details>
      <section className="settings-section">
        <div>
          <h2>Аккаунт и синхронизация</h2>
          <p>Профиль, сессия и синхронизация вашей музыки.</p>
        </div>
        <div className="settings-panel">
          <details>
            <summary>Профили на этом устройстве</summary>
            <Profiles runtime={runtime} />
          </details>
          <details>
            <summary>Подключение Telegram</summary>
            <Authorization key={session_id} runtime={runtime} />
          </details>
          <details>
            <summary>Музыкальная библиотека</summary>
            <VaultSetup key={session_id} runtime={runtime} />
          </details>
          <button
            className="danger-text"
            disabled={
              !!runtime.busy || model.jobs.some((j) => j.state === "running")
            }
            onClick={() => setLogout(true)}
          >
            Выйти из Telegram
          </button>
        </div>
      </section>
      <section className="settings-section">
        <div>
          <h2>Библиотека</h2>
          <p>
            {Object.keys(model.projection?.state.tracks ?? {}).length}{" "}
            композиций. Резервные копии не содержат Telegram-сессию.
          </p>
        </div>
        <div className="settings-panel">
          <button
            disabled={!model.projection}
            onClick={() =>
              void run("Экспорт резервной копии", async () => {
                const path = await save({
                  defaultPath: "kiyora-backup.tar",
                  filters: [{ name: "Архив", extensions: ["tar"] }],
                });
                if (path)
                  await call("export_backup", {
                    session_id,
                    path,
                    kind: "metadata",
                  });
              })
            }
          >
            Экспортировать каталог
          </button>
          <button
            disabled={!online}
            onClick={() =>
              void run(
                "Выбор резервной копии",
                async () => {
                  const path = await open({
                    multiple: false,
                    filters: [{ name: "Архив", extensions: ["tar"] }],
                  });
                  if (typeof path === "string") setRestoring(path);
                },
                false,
              )
            }
          >
            Восстановить из резервной копии
          </button>
          <button
            disabled={!online}
            onClick={() =>
              void run("Снимок библиотеки", () =>
                call("vault", { session_id, request: { kind: "snapshot" } }),
              )
            }
          >
            Сохранить резервную копию в облаке
          </button>
        </div>
      </section>
      <section className="settings-section">
        <div>
          <h2>О приложении</h2>
          <p>Личное музыкальное облако.</p>
        </div>
        <div className="settings-panel">
          <strong className="wordmark">KIYORA</strong>
          <p>Версия {packageInfo.version}</p>
          <button
            onClick={() =>
              void run(
                "Открыть GitHub",
                () => openUrl("https://github.com/Arkfarin/kiyora"),
                false,
              )
            }
          >
            <ExternalLink />
            Исходный код
          </button>
          <p className="small muted">
            React · Tauri · Rust · TDLib · SQLite · Lucide
          </p>
          <details>
            <summary>Шрифты и лицензии</summary>
            <p>
              Manrope — Mikhail Sharanda, Mirko Velimirovic. Space Grotesk —
              Florian Karsten. SIL Open Font License 1.1.
            </p>
            <a
              href="/licenses/Manrope-OFL.txt"
              target="_blank"
              rel="noreferrer"
            >
              Лицензия Manrope
            </a>
            <br />
            <a
              href="/licenses/SpaceGrotesk-OFL.txt"
              target="_blank"
              rel="noreferrer"
            >
              Лицензия Space Grotesk
            </a>
          </details>
        </div>
      </section>
      <section className="settings-section">
        <div>
          <h2>Диагностика</h2>
          <p>
            Отчёт исключает имена, идентификаторы аккаунтов, пути, токены и
            содержимое музыки.
          </p>
        </div>
        <div className="settings-panel">
          <button
            onClick={() =>
              void run(
                "Копирование диагностики",
                async () => {
                  await navigator.clipboard.writeText(
                    JSON.stringify(report, null, 2),
                  );
                  setCopy(true);
                },
                false,
              )
            }
          >
            {copy ? <Check /> : <Copy />}
            {copy ? "Скопировано" : "Копировать обезличенный отчёт"}
          </button>
          <details>
            <summary>Состояние приложения</summary>
            <pre className="diagnostics">{JSON.stringify(report, null, 2)}</pre>
          </details>
        </div>
      </section>
      {logout && (
        <Confirm
          title="Выйти из Telegram?"
          confirmLabel="Выйти"
          busy={!!runtime.busy}
          onClose={() => setLogout(false)}
          onConfirm={() =>
            void run("Выход из Telegram", async () => {
              await call("logout", { session_id });
              setLogout(false);
            })
          }
        >
          Для следующего подключения потребуется авторизация. Музыка в Telegram
          останется на месте.
        </Confirm>
      )}
      {restoring && (
        <Confirm
          title="Восстановить каталог?"
          confirmLabel="Проверить и восстановить"
          busy={!!runtime.busy}
          onClose={() => setRestoring(null)}
          onConfirm={() =>
            void run("Восстановление каталога", async () => {
              await call("vault", {
                session_id,
                request: {
                  kind: "import_backup",
                  payload: { path: restoring },
                },
              });
              setRestoring(null);
            })
          }
        >
          Приложение проверит архив перед восстановлением. Восстановление может
          добавить данные в подключённую библиотеку.
        </Confirm>
      )}
    </>
  );
}
