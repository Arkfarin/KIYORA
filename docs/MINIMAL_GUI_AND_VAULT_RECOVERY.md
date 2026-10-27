# Kiyora: минимальный GUI и восстановление Vault

Изменения выполнены только в исходниках. Тесты, сборки, форматирование, статические
проверки, приложение и Telegram API при этой работе не запускались. Пользовательские
SQLite, профили, TDLib-сессии и Telegram-каналы не открывались и не изменялись.
Работоспособность после компиляции и на реальном аккаунте не подтверждена.

## Найденные причины

1. **Integrity после VAULT_INIT и BARRIER.** Перед первой загрузкой snapshot
   `TdTransport::validate_upload` вызывает `upload_limit`. Тот принимал только
   `optionValueBoolean` для `getOption("is_premium")`. В закреплённых исходниках
   TDLib 1.8.69 `UserManager::update_user` устанавливает эту опцию, только если
   значение отличается от `get_option_boolean` с default=false. Для обычного
   аккаунта опция может оставаться пустой. `OptionManager` возвращает
   `optionValueEmpty`, который адаптер ошибочно превращал в `InvalidResponse`,
   затем в общий `Integrity`. Это конкретный путь отказа до публикации snapshot A,
   соответствующий последовательности INIT → BARRIER → ошибка. Теперь Empty
   означает консервативный обычный лимит; неизвестные типы по-прежнему отклоняются.
   Это вывод из исходников, а не подтверждение состояния конкретного аккаунта.

2. **SYNCING / NoVault между вызовами CLI.** Каждый процесс восстанавливает
   профиль, но TDLib может ещё переходить к Ready, а сервис стартует асинхронно.
   `service_request` раньше сразу возвращал NoVault при `service == None`, даже
   при сохранённом binding. Начальный статус уже созданного сервиса, напротив,
   содержал SYNCING и `vault: null` до первого прохода. Теперь общий backend
   ожидает реальную авторизацию и восстанавливает сервис перед командой.
   CLI status/sync status ожидают первый результат; attach ожидает первый аудит.
   Локальные чтения GUI не блокируются на этом ожидании. Отсутствующий binding,
   неполный snapshot, отсутствие авторизации и повреждённая БД различаются.

3. **Невозможное продолжение создания.** `resume_created_channel` допускал только
   INIT и отвергал BARRIER/SNAPSHOT/COMMIT. Кроме того, он перезаписывал chat_id
   intent до проверки и запускал Sync Engine до публикации INIT. Теперь chat_id
   и vault_id проверяются, подтверждённый канал журналируется, существующий INIT
   переиспользуется, а snapshot продолжает существующий Sync Engine.

4. **Неидемпотентный S0.** Initialize создавал новый barrier при каждом повторе,
   а publish заново отправлял обе копии. Теперь используется существующий barrier,
   проверяется его read-back, а отправки журналируются в существующем local_settings.
   Snapshot A/B/commit ищутся по идентичности и проверяются до повторной публикации.
   Неизвестная отправка без найденного сообщения блокирует повтор; подтверждённый
   отсутствующий barrier не заменяется новым. Непустой uncommitted vault нельзя
   превратить в пустой S0. Найденный commit всегда ведёт к обычной синхронизации.
   Creation intent снимается только после ONLINE_READY.

5. **Повторное восстановление профиля и контекст jobs.** Восстановление теперь
   использует тот же transition lock, что и переключение профиля. Повторный выбор
   уже активного профиля возвращает существующую сессию. Выбор сохранённого профиля
   читается под lock. После первого attach/create контекст фоновых jobs обновляется
   вместе с vault_id, иначе frontend мог отфильтровать их как события другого Vault.

## Интерфейс и runtime

Tauri сохраняет существующий managed `Arc<Backend>`. WebView bootstrap идемпотентен;
React удаляет только свои listeners. Закрытие Rust-приложения, явное завершение
Telegram-сессии, смена профиля и жизненный цикл аудио остаются отдельными операциями.

Добавлены Library, Playlists, Imports, Settings и Debug / Diagnostics:

- создание/выбор профиля, восстановление защищённых credentials;
- телефон, QR с обновлениями, код, 2FA, email и email code;
- discover, ручной attach, явное создание и продолжение инициализации;
- реальные фазы create/sync, без искусственного прогресса и без автоматического
  создания канала после ошибки;
- SQLite snapshot библиотеки, поиск, страницы, обложки, статус локального кэша;
- создание/открытие плейлиста, добавление/удаление записей, воспроизведение;
- импорт файлов через абсолютный путь или native Tauri drag-and-drop, durable
  jobs, отмена и возобновление исходного запроса с сохранёнными UUID;
- Rust PlaybackEngine, очередь, автоматический выбор source_id с предпочтением
  кэша, play/pause/resume/stop/seek/volume/previous/next;
- download-for-offline через существующий сервис;
- диагностика с allowlist полей: режим, профиль, сессия, Vault/chat, TDLib,
  Sync Engine, ошибки, jobs, импорты, playback и последние события Rust.

Подписки `kiyora://backend` и `kiyora://resync-required` устанавливаются до первого
снимка. Последовательности сравниваются как BigInt; snapshot watermarks,
profile/session/vault context, буфер на время чтения и повторное чтение после
overrun защищают от старых событий. Fallback выполняется раз в 60 секунд и при
возврате фокуса. Аудиопозиция поступает из событий Rust, а не из HTML audio.
Выгрузка диагностики не сериализует весь React model, auth/QR или формы ввода.

Versioned `backend_invoke` остаётся основным IPC. Дополнительные Tauri-команды:
`backend_bootstrap`, `backend_session_info`, `backend_reconnect`, `backend_auth_qr`,
`backend_resume_job`. Они вызывают тот же Backend. В event contract добавлены
Operation=create и фазы creating_channel/publishing_init/publish_commit.
TMV/1, SQLite schema и существующие миграции не изменены. Новые зависимости не
добавлены; QR использует уже имеющийся Rust qrcode.

## Ручной запуск на CachyOS

Команды ниже предназначены для самостоятельного выполнения, здесь они не запускались.
Нужны Node/pnpm, Rust toolchain проекта, GTK3, WebKitGTK 4.1, libsoup3, ALSA,
OpenSSL, D-Bus и работающее Secret Service хранилище (например, разблокированный
KWallet с Secret Service или GNOME Keyring). Для сборки native-зависимостей нужны
также C/C++ toolchain, CMake, pkgconf и gperf; см. существующий `src-tauri/docs/tdlib.md`.

Из корня репозитория, если зависимости ещё не установлены:

```bash
pnpm install --frozen-lockfile
```

Используйте **уже подготовленную закреплённую TDLib 1.8.69**. Для стандартного
пути из существующего build-tdlib.sh:

```bash
export KIYORA_TDJSON="$PWD/src-tauri/target/native/td-build/libtdjson.so"
pnpm tauri dev
```

Если библиотека находится иначе, замените KIYORA_TDJSON на её абсолютный путь.
Не запускайте CLI с тем же профилем одновременно с GUI: lock вернёт PROFILE_BUSY.
`pnpm tauri dev` самостоятельно запустит frontend и сборку Tauri.

Для описанного неполного Vault откройте Settings, дождитесь авторизации и используйте
сохранённый binding либо подключите chat_id `-1004483445887`. Сверьте Vault ID
`805cedc7-f760-4315-a6a7-1e5dd1addba7`, затем нажмите «Продолжить инициализацию».
Это действие не создаёт канал. ONLINE_READY выставляется только после проверки
snapshot и согласования server head. При Integrity/Timeout сохраните диагностику;
не удаляйте SQLite/intent/session и не создавайте второй канал.

## Ограничения

- Компиляция, типы, платформы, native-аудио и Telegram-поведение не проверялись
  по прямому требованию задачи. Здесь нет утверждения об успешной проверке.
- Отдельный native file picker не добавлен: desktop использует drag-and-drop и
  абсолютный путь. Android resolver content:// остаётся в backend, но GUI выбора
  SAF-документа в этой реализации не добавлен.
- Регистрация нового Telegram-аккаунта и Premium-required шаг не реализованы
  существующим backend; UI показывает точное состояние и предлагает официальный
  клиент, не имитируя успешную авторизацию.
- Неизвестный исход отправки без доступного server record намеренно остаётся
  заблокированным. Telegram не даёт атомарности или доказательства отсутствия
  сообщения после потерянного ACK. Автоматический сброс журнала не выполняется.
- Начальное восстановление использует полный локальный state_snapshot; интерфейс
  библиотеки разбит на страницы. Imports показывает до 500 записей журнала.
- Журнал диагностических событий ограничен последними 100 записями в Rust-процессе;
  после полного завершения приложения эта история не сохраняется на диск.

## Изменённые файлы

- `index.html`, `src/App.tsx`, `src/App.css` — shell и тема.
- `src/backend.ts`, `src/useBackend.ts` — типы, IPC и событийное состояние.
- `src/Setup.tsx`, `src/Music.tsx` — авторизация/Vault, музыка, jobs и плеер.
- `src-tauri/src/lib.rs`, `src-tauri/tauri.conf.json` — Tauri IPC и окно.
- `src-tauri/crates/vault-core/src/backend.rs` и `backend/runtime.rs` — lifecycle,
  resume, bootstrap/status и QR.
- `src-tauri/crates/vault-core/src/api.rs`, `events.rs`, `jobs.rs`, `service.rs` —
  диагностические ошибки, события, возобновление jobs, статус actor.
- `src-tauri/crates/vault-core/src/bin/kiyora-cli.rs` — ожидание начального статуса.
- `src-tauri/crates/vault-sync/src/engine.rs`, `progress.rs` — S0 recovery и фазы.
- `src-tauri/crates/vault-snapshot/src/lib.rs` — переиспользование публикаций A/B/commit.
- `src-tauri/crates/vault-telegram/src/transport.rs` — default is_premium.
- `src-tauri/spec/schema/backend-event-v1.schema.json` — дополнительные фазы событий.
- `docs/MINIMAL_GUI_AND_VAULT_RECOVERY.md` — этот отчёт и ручной запуск.
