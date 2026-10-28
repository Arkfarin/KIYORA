import { useState } from "react";
import {
  ArrowRight,
  Check,
  Cloud,
  Headphones,
  ShieldCheck,
} from "lucide-react";
import { isTauri } from "@tauri-apps/api/core";
import type { Runtime } from "../../useBackend";
import { Profiles, Authorization, VaultSetup, phaseNames } from "../../Setup";
import { Loading } from "../../components/ui/Primitives";
export function Onboarding({
  runtime,
  openLocal,
}: {
  runtime: Runtime;
  openLocal: () => void;
}) {
  const { model, refresh } = runtime;
  const [started, setStarted] = useState(false);
  const hasProfile = !!model.status;
  const hasVault = !!model.status?.context.vault_id;
  const authorized = model.auth?.state === "READY";
  if (model.loading)
    return (
      <div className="boot-screen">
        <span className="wordmark">KIYORA</span>
        <Loading label="Восстанавливаем ваш профиль…" />
        <p>Сохранённая библиотека останется на месте.</p>
      </div>
    );
  if (!isTauri())
    return (
      <div className="welcome">
        <div className="welcome-copy">
          <h1 className="wordmark">KIYORA</h1>
          <h2>
            Ваша музыка.
            <br />
            Ваше облако.
            <br />
            Ваши правила.
          </h2>
          <p>
            Личная музыкальная коллекция в Telegram. Для входа и прослушивания
            откройте Kiyora на компьютере или Android.
          </p>
          <p className="native-note">
            Веб-предпросмотр интерфейса. Нативный backend недоступен в обычном
            браузере.
          </p>
        </div>
        <div className="welcome-side">
          <Headphones size={48} strokeWidth={1} />
          <p>
            Музыка, которая
            <br />
            принадлежит вам.
          </p>
          <span>KIYORA</span>
        </div>
      </div>
    );
  if (!hasProfile && !model.profiles.length && !started && !model.error)
    return (
      <div className="welcome">
        <div className="welcome-copy">
          <h1 className="wordmark">KIYORA</h1>
          <h2>
            Ваша музыка.
            <br />
            Ваше облако.
            <br />
            Ваши правила.
          </h2>
          <p>
            Храните любимые композиции в личном Telegram Vault. Слушайте их
            здесь — дома, в дороге, в своём ритме.
          </p>
          <button className="primary" onClick={() => setStarted(true)}>
            Начать настройку
            <ArrowRight />
          </button>
          <div className="welcome-points">
            <span>
              <ShieldCheck />
              Приватная коллекция
            </span>
            <span>
              <Cloud />
              Ваш Telegram
            </span>
          </div>
        </div>
        <div className="welcome-side">
          <Headphones size={48} strokeWidth={1} />
          <p>
            Всё начинается
            <br />с вашей музыки.
          </p>
          <span>KIYORA</span>
        </div>
      </div>
    );
  const step = !hasProfile ? 0 : !authorized ? 1 : !hasVault ? 2 : 3;
  return (
    <div className="setup-screen">
      <header>
        <span className="wordmark">KIYORA</span>
        <span className="muted">Ваше музыкальное пространство</span>
      </header>
      <div className="setup-layout">
        <aside>
          <h1>{hasVault ? "Возвращаемся к музыке" : "Настроим Kiyora"}</h1>
          <p>
            {hasVault
              ? "Сохранённый Vault найден. Восстанавливаем подключение к вашей коллекции."
              : "Подключите Telegram и выберите своё музыкальное хранилище."}
          </p>
          <ol className="setup-steps">
            {[
              "Локальный профиль",
              "Вход в Telegram",
              "Музыкальное хранилище",
              "Синхронизация",
            ].map((name, i) => (
              <li
                key={name}
                className={step === i ? "current" : step > i ? "done" : ""}
              >
                {step > i ? <Check /> : <span className="step-dot" />}
                {name}
              </li>
            ))}
          </ol>
          {hasVault && model.projection && (
            <button onClick={openLocal}>
              Открыть локальную библиотеку
              <ArrowRight />
            </button>
          )}
        </aside>
        <div className="setup-form">
          {model.error && (
            <div className="inline-error" role="alert">
              <strong>Не удалось завершить подключение</strong>
              <p>{model.error.message}</p>
              <button onClick={() => void refresh()}>
                Проверить состояние
              </button>
            </div>
          )}
          {!hasProfile ? (
            <Profiles runtime={runtime} />
          ) : !authorized ? (
            <Authorization
              key={model.status!.context.session_id}
              runtime={runtime}
            />
          ) : !hasVault || model.info?.creation ? (
            <VaultSetup runtime={runtime} />
          ) : (
            <>
              <Loading
                label={
                  model.progress
                    ? (phaseNames[model.progress.phase] ??
                      "Синхронизация библиотеки…")
                    : "Восстанавливаем библиотеку…"
                }
              />
              {model.progress?.total != null && (
                <progress
                  max={Math.max(1, model.progress.total)}
                  value={model.progress.completed}
                  aria-label="Синхронизация"
                />
              )}
              <VaultSetup runtime={runtime} />
            </>
          )}
          {hasProfile && step !== 0 && (
            <details>
              <summary>Управление профилями</summary>
              <Profiles runtime={runtime} />
            </details>
          )}
        </div>
      </div>
    </div>
  );
}
