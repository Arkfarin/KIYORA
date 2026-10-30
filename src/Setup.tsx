import { useEffect, useRef, useState } from "react";
import type { FormEvent } from "react";
import { authQr, call, reconnect } from "./backend";
import type { VaultCandidate } from "./backend";
import type { Runtime } from "./useBackend";

export function Profiles({ runtime }: { runtime: Runtime }) {
  const { model, run, busy } = runtime;
  const [label, setLabel] = useState("");
  const create = (event: FormEvent) => {
    event.preventDefault();
    void run("Создание профиля", async () => {
      const profile = await call("create_profile", { label: label.trim() });
      await call("switch_profile", { profile_id: profile.profile_id });
      setLabel("");
    });
  };
  return (
    <section className="panel">
      <h2>Профиль</h2>
      <p className="muted">
        Каждый профиль хранит свою Telegram-сессию и локальную библиотеку.
      </p>
      {model.profiles.length > 0 && (
        <label>
          Текущий профиль
          <select
            value={model.status?.context.profile_id ?? ""}
            disabled={
              !!busy || model.jobs.some((job) => job.state === "running")
            }
            onChange={(e) =>
              void run("Открытие профиля", () =>
                call("switch_profile", { profile_id: e.target.value }),
              )
            }
          >
            <option value="" disabled>
              Выберите профиль
            </option>
            {model.profiles.map((p) => (
              <option key={p.profile_id} value={p.profile_id}>
                {p.label}
              </option>
            ))}
          </select>
        </label>
      )}
      <form className="inline-form" onSubmit={create}>
        <label>
          Новый профиль
          <input
            value={label}
            onChange={(e) => setLabel(e.target.value)}
            maxLength={100}
            placeholder="Личный"
            required
          />
        </label>
        <button disabled={!!busy || !label.trim()}>Создать профиль</button>
      </form>
    </section>
  );
}

export function Authorization({ runtime }: { runtime: Runtime }) {
  const { model, run, busy } = runtime;
  const [apiId, setApiId] = useState("");
  const [apiHash, setApiHash] = useState("");
  const [input, setInput] = useState("");
  const [qr, setQr] = useState<string | null>(null);
  const [qrError, setQrError] = useState(false);
  const [now, setNow] = useState(Date.now());
  const [resendAt, setResendAt] = useState(0);
  const session_id = model.status?.context.session_id;
  const auth = model.auth;
  useEffect(() => {
    setInput("");
    setApiHash("");
  }, [session_id, auth?.state]);
  useEffect(() => {
    setResendAt(Date.now() + (auth?.code_info?.timeout_seconds ?? 0) * 1000);
  }, [
    auth?.state,
    auth?.code_info?.timeout_seconds,
    auth?.code_info?.delivery,
  ]);
  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, []);
  useEffect(() => {
    let active = true;
    setQr(null);
    setQrError(false);
    if (session_id && auth?.state === "QR_CONFIRMATION") {
      void authQr(session_id)
        .then((svg) => {
          if (active && svg)
            setQr(
              `data:image/svg+xml;charset=utf-8,${encodeURIComponent(svg)}`,
            );
        })
        .catch(() => {
          if (active) setQrError(true);
        });
    }
    return () => {
      active = false;
    };
  }, [session_id, auth?.state, auth?.link]);
  if (!session_id) return null;
  const send = (kind: string, value?: string) => {
    const ephemeral = value;
    setInput("");
    void run("Авторизация Telegram", () =>
      call("authenticate", {
        session_id,
        input: ephemeral === undefined ? { kind } : { kind, value: ephemeral },
      }),
    );
  };
  const kind = (
    {
      PHONE_REQUIRED: "phone",
      CODE_REQUIRED: "code",
      PASSWORD_REQUIRED: "password",
      EMAIL_REQUIRED: "email",
      EMAIL_CODE_REQUIRED: "email_code",
    } as Record<string, string>
  )[auth?.state ?? ""];
  const label = (
    {
      phone: "Номер телефона",
      code: "Код Telegram",
      password: "Облачный пароль 2FA",
      email: "Email",
      email_code: "Код из email",
    } as Record<string, string>
  )[kind];
  const remaining = Math.max(0, Math.ceil((resendAt - now) / 1000));
  return (
    <section className="panel">
      <div className="section-title">
        <h2>Telegram</h2>
        <span className={`badge ${auth?.state === "READY" ? "good" : ""}`}>
          {auth?.state ?? "Загрузка"}
        </span>
      </div>
      {auth?.state === "READY" ? (
        <p className="muted">
          Сессия авторизована. Готовность облачных изменений определяется
          состоянием библиотеки.
        </p>
      ) : (
        <>
          {(!auth ||
            ["PARAMETERS_REQUIRED", "CLOSED", "CLOSING"].includes(
              auth.state,
            )) && (
            <>
              {model.info?.api_configured && (
                <button
                  disabled={!!busy}
                  onClick={() =>
                    void run("Подключение Telegram", () =>
                      reconnect(session_id),
                    )
                  }
                >
                  Подключить сохранённую сессию
                </button>
              )}
              <form
                onSubmit={(event) => {
                  event.preventDefault();
                  const secret = apiHash;
                  setApiHash("");
                  void run("Подключение Telegram", () =>
                    call("connect_telegram", {
                      session_id,
                      api_id: Number(apiId),
                      api_hash: secret,
                    }),
                  );
                }}
                className="stack"
              >
                <label>
                  API ID
                  <input
                    value={apiId}
                    inputMode="numeric"
                    onChange={(e) => setApiId(e.target.value)}
                    pattern="[0-9]+"
                    required
                    autoComplete="off"
                  />
                </label>
                <label>
                  API hash
                  <input
                    type="password"
                    value={apiHash}
                    onChange={(e) => setApiHash(e.target.value)}
                    pattern="[0-9a-fA-F]{32}"
                    required
                    autoComplete="off"
                    spellCheck={false}
                  />
                </label>
                <p className="muted small">
                  Получите API ID и API hash в разделе API development tools на
                  my.telegram.org. Данные сохраняются в системном хранилище
                  секретов и не попадают в браузерное хранилище.
                </p>
                <button className="primary" disabled={!!busy}>
                  Подключить
                </button>
              </form>
            </>
          )}
          {kind && (
            <form
              className="stack"
              onSubmit={(event) => {
                event.preventDefault();
                send(kind, input);
              }}
            >
              <label>
                {label}
                <input
                  value={input}
                  onChange={(e) => setInput(e.target.value)}
                  type={
                    kind === "password"
                      ? "password"
                      : kind === "email"
                        ? "email"
                        : "text"
                  }
                  autoComplete="off"
                  spellCheck={false}
                  placeholder={kind === "phone" ? "+996…" : undefined}
                  required
                />
              </label>
              {auth?.code_info && (
                <p className="muted small">
                  Доставка: {auth.code_info.delivery}
                  {auth.code_info.expected_length
                    ? ` · ${auth.code_info.expected_length} символов`
                    : ""}
                </p>
              )}
              <div className="actions">
                <button className="primary" disabled={!!busy || !input}>
                  Продолжить
                </button>
                {kind === "phone" && (
                  <button
                    type="button"
                    disabled={!!busy}
                    onClick={() => send("qr")}
                  >
                    Войти по QR
                  </button>
                )}
                {kind === "code" && auth?.code_info?.next_delivery && (
                  <button
                    type="button"
                    disabled={!!busy || remaining > 0}
                    onClick={() => send("resend_code")}
                  >
                    {remaining
                      ? `Повторить через ${remaining} с`
                      : "Отправить код повторно"}
                  </button>
                )}
              </div>
            </form>
          )}
          {auth?.state === "QR_CONFIRMATION" && (
            <div className="qr-login">
              {qr ? (
                <img className="qr" src={qr} alt="QR-код входа в Telegram" />
              ) : (
                <p>
                  {qrError
                    ? "Не удалось отобразить QR. Запросите обновление."
                    : "Подготовка QR…"}
                </p>
              )}
              <p>
                Telegram на телефоне → Настройки → Устройства → Подключить
                устройство.
              </p>
              <button disabled={!!busy} onClick={() => send("qr")}>
                Обновить QR
              </button>
              <p className="muted small">QR-код обновляется автоматически.</p>
            </div>
          )}
          {auth &&
            [
              "REGISTRATION_REQUIRED",
              "PREMIUM_REQUIRED",
              "UNSUPPORTED",
            ].includes(auth.state) && (
              <p>
                Telegram требует дополнительный шаг ({auth.state}). Текущий
                приложение пока не поддерживает его ввод; завершите требование в
                официальном клиенте и повторите вход.
              </p>
            )}
        </>
      )}
    </section>
  );
}

export const phaseNames: Record<string, string> = {
  creating_channel: "Создаём приватный канал",
  publishing_init: "Готовим библиотеку",
  barrier: "Фиксируем границу синхронизации",
  encode: "Готовим снимок библиотеки",
  publish_copies: "Сохраняем две копии снимка",
  publish_commit: "Подтверждаем снимок",
  verify: "Проверяем состояние",
  replay: "Применяем изменения",
  reconcile_head: "Проверяем актуальность",
  complete: "Операция завершена",
};
export function VaultSetup({ runtime }: { runtime: Runtime }) {
  const { model, run, busy } = runtime;
  const [chat, setChat] = useState("");
  const [candidates, setCandidates] = useState<VaultCandidate[] | null>(null);
  const [title, setTitle] = useState("");
  const session_id = model.status?.context.session_id;
  const ready = model.auth?.state === "READY";
  const discovered = useRef<string | undefined>(undefined);
  useEffect(() => {
    if (
      !session_id ||
      !ready ||
      model.status?.context.vault_id ||
      model.info?.creation ||
      discovered.current === session_id
    )
      return;
    discovered.current = session_id;
    void run("Поиск вашей библиотеки", async () =>
      setCandidates(await call("discover_vaults", { session_id })),
    );
  }, [
    session_id,
    ready,
    model.status?.context.vault_id,
    model.info?.creation,
    run,
  ]);
  const boundChat = model.info?.chat_id ?? model.info?.creation?.chat_id;
  useEffect(() => {
    if (boundChat) setChat(boundChat);
  }, [boundChat]);
  useEffect(() => {
    setCandidates(null);
    setChat("");
  }, [session_id]);
  if (!session_id) return null;
  const validChat = /^-?[1-9]\d*$/.test(chat);
  const locked = !!model.status?.context.vault_id || !!model.info?.creation;
  const recover = () =>
    void run("Восстановление инициализации", () =>
      call("resume_vault_creation", { session_id, chat_id: boundChat ?? chat }),
    );
  return (
    <section className="panel">
      <div className="section-title">
        <h2>Музыкальная библиотека</h2>
        <span
          className={`badge ${model.status?.mode === "ONLINE_READY" ? "good" : ""}`}
        >
          {(
            {
              ONLINE_READY: "Подключена",
              SYNCING: "Обновляется",
              RECONNECTING: "Нет подключения",
              AUTH_REQUIRED: "Нужен вход",
              DISCOVERING: "Поиск библиотеки",
              DEGRADED_READONLY: "Только чтение",
            } as Record<string, string>
          )[model.status?.mode ?? ""] ?? "Подключение"}
        </span>
      </div>
      {model.status?.context.vault_id && (
        <details>
          <summary>Сведения о подключении</summary>
          <p className="mono small">
            {model.status.context.vault_id}
            <br />
            Канал: {boundChat ?? "—"}
          </p>
        </details>
      )}
      {model.info?.creation && (
        <p className="notice">
          Настройка не завершена:{" "}
          {phaseNames[model.info.creation.stage] ?? "готовим библиотеку"}.
          Продолжение использует этот же канал.
        </p>
      )}
      {model.progress && (
        <p className="progress-label" role="status">
          {phaseNames[model.progress.phase] ?? "Обновляем библиотеку"}
          {model.progress.total !== null
            ? ` · ${model.progress.completed}/${model.progress.total}`
            : ""}
        </p>
      )}
      <div className="actions">
        <button
          disabled={!ready || !!busy}
          onClick={() =>
            void run("Поиск библиотеки", async () =>
              setCandidates(await call("discover_vaults", { session_id })),
            )
          }
        >
          Подключить существующее
        </button>
        <button
          disabled={!ready || !!busy}
          onClick={() =>
            void run("Расширенный поиск библиотеки", async () =>
              setCandidates(await call("discover_vaults_deep", { session_id })),
            )
          }
        >
          Расширенный поиск
        </button>
        {model.status?.context.vault_id && (
          <button
            disabled={!ready || !!busy}
            onClick={() =>
              void run("Синхронизация", () =>
                call("vault", { session_id, request: { kind: "sync" } }),
              )
            }
          >
            Синхронизировать
          </button>
        )}
        {(boundChat || model.info?.creation) && (
          <button
            disabled={!ready || !!busy || !(boundChat || validChat)}
            onClick={recover}
          >
            Продолжить настройку
          </button>
        )}
      </div>
      {candidates?.length === 0 && (
        <p className="muted">
          Библиотека пока не найдена. Попробуйте расширенный поиск или
          подключите канал вручную, если знаете его идентификатор.
        </p>
      )}
      {candidates?.map((candidate) => (
        <div className="candidate" key={candidate.channel.chat_id}>
          <div>
            <strong>{candidate.channel.title}</strong>
            <p className="mono small">
              {candidate.channel.chat_id} · schema {candidate.protocol}
              <br />
              {candidate.init.vault_id}
            </p>
          </div>
          <button
            disabled={
              !!busy ||
              !ready ||
              (!!boundChat && boundChat !== candidate.channel.chat_id)
            }
            onClick={() =>
              void run("Подключение библиотеки", () =>
                call("attach_vault", {
                  session_id,
                  chat_id: candidate.channel.chat_id,
                }),
              )
            }
          >
            Подключить
          </button>
        </div>
      ))}
      <form
        className="inline-form"
        onSubmit={(event) => {
          event.preventDefault();
          void run("Подключение библиотеки", () =>
            call("attach_vault", { session_id, chat_id: chat }),
          );
        }}
      >
        <label>
          Chat ID
          <input
            value={chat}
            onChange={(e) => setChat(e.target.value.trim())}
            placeholder="-100…"
            spellCheck={false}
          />
        </label>
        <button
          disabled={
            !ready ||
            !!busy ||
            !validChat ||
            (!!boundChat && boundChat !== chat)
          }
        >
          Подключить
        </button>
        {model.info?.creation && !boundChat && (
          <button
            type="button"
            disabled={!ready || !!busy || !validChat}
            onClick={recover}
          >
            Восстановить созданный канал
          </button>
        )}
      </form>
      {!locked && (
        <details>
          <summary>Создать новую библиотеку</summary>
          <p className="muted">
            Создаёт новый приватный канал. Для существующего хранилища
            используйте поиск или chat_id.
          </p>
          <form
            className="inline-form"
            onSubmit={(event) => {
              event.preventDefault();
              void run("Создание библиотеки", () =>
                call("create_vault", {
                  session_id,
                  ...(title.trim() ? { title: title.trim() } : {}),
                }),
              );
            }}
          >
            <label>
              Название (необязательно)
              <input
                value={title}
                onChange={(e) => setTitle(e.target.value)}
                maxLength={128}
              />
            </label>
            <button className="primary" disabled={!ready || !!busy}>
              Создать библиотеку
            </button>
          </form>
        </details>
      )}
      {model.info?.repair_required && (
        <div className="notice">
          <p>
            Локальная SQLite повреждена. Восстановление проверяет облачный
            резервную копию и сохраняет исходные файлы.
          </p>
          <button
            disabled={!ready || !!busy}
            onClick={() =>
              void run("Восстановление SQLite", () =>
                call("repair_local_database", { session_id }),
              )
            }
          >
            Восстановить из резервной копии
          </button>
        </div>
      )}
    </section>
  );
}
