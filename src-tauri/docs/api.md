**Backend API v1**

Основная Tauri-команда — `backend_invoke`. Она принимает `request: BackendRequest`
и возвращает `BackendResponse`. Типы определены в `vault-core/src/ipc.rs`; эти же
типы используются для исполнения запроса и генерации JSON Schema. Существующие
команды `backend_*` сохранены для совместимости. Frontend не изменён.

Пример аргумента `request` для получения состава плейлиста:

```json
{
  "version": 1,
  "request": {
    "command": "query",
    "args": {
      "session_id": "11111111-1111-4111-8111-111111111111",
      "query": {
        "kind": "playlist",
        "payload": {
          "playlist_id": "22222222-2222-4222-8222-222222222222",
          "offset": 0,
          "limit": 100
        }
      }
    }
  }
}
```

Ответ содержит `version: 1` и `result: {"Ok": ...}` либо
`result: {"Err": {"code": ..., "message": ..., "retryable": ...}}`.
Неизвестная версия отвергается до выполнения операции. Telegram chat/user/message
IDs передаются десятичными строками. UUID идентифицируют профиль, текущую сессию,
объекты и операции. При смене профиля создаётся новый `session_id`; вызов со старым
ID получает `CONFLICT`.

Вызовы охватывают профили, авторизацию, discovery/attach/create/resume vault,
библиотеку, плейлисты, локальные запросы, операции vault, экспорт backup и playback.
Полные варианты и поля: `spec/schema/backend-request-v1.schema.json`,
`backend-response-v1.schema.json`, `backend-event-v1.schema.json`. События приходят
через `kiyora://backend`, содержат context и монотонный `sequence`.

Локальные запросы `playlist`, `command_status`, `favorite_sets`, `favorites`,
`settings`, `device`, `albums`, `artists`, `imports` доступны через `query`.
Они не требуют Telegram-сессии. Для данных библиотеки нужна локальная проекция
выбранного vault. Кэш и `device` доступны даже до привязки vault. Размер страницы
ограничивается 500. Плейлист возвращает `total`, `offset`, записи с полными треками
в доменном порядке `after_entry_id`, включая повторяющиеся треки.

Операции кэша: `get_cache_stats`, `clear_local_cache`, `pin_cached_media`,
`get_download_status`, `set_cache_quota`. Квота сохраняется для профиля; уменьшение
не вытесняет закреплённые/используемые файлы и резервы активных загрузок. Учёт
отдельного TDLib files storage ещё требует завершения (см. remediation-status).

GC-роль назначается `assign_gc_coordinator`: `target` = `this_device`,
`device` с `device_id` либо `disabled`; передаются `op_id`, `expected_previous`
и `force_confirmed`. Обычная передача выполняется старым координатором после
завершения или `abandon_gc` незавершённых планов. Принудительная передача требует
явного `force_confirmed: true` и аннулирует локальные планы. Это роль приложения,
а не серверная блокировка Telegram. Нельзя гарантировать остановку уже принятого
Telegram запроса от старого координатора. Перед удалением нужен новый проверенный
план с подтверждением его digest; проверки роли и снимков повторяются по батчам.

`share_track` требует стабильный `op_id` для одного намерения пользователя.
Повторные нажатия и восстановление запроса должны использовать тот же UUID.
Ответ `shared` содержит `state`: `sent`, `not_sent` или `unknown`; только `sent`
содержит подтверждённый `message_id`. `get_share_status {op_id}` читает сохранённый
результат без отправки. Один `op_id` никогда не отправляется повторно, даже после
перезапуска. `unknown` не означает неудачу: Telegram мог принять файл, а ответ
потеряться. Проверять чат получателя нужно до нового явно выбранного отправления
с новым UUID. После `not_sent` новый запрос также требует нового UUID.
CLI: `library share <track> <source> <chat> [--op-id <uuid>]`; идентификатор выводится
до отправки. Журнал локален для vault/профиля, а не является распределённой
дедупликацией между независимыми устройствами или после удаления локальной БД.

Входящий `messageAudio` копируется на сервере с новым MEDIA-marker: аудиобайты
не скачиваются, title/performer/duration берутся из TDLib.
Миниатюра обложки при наличии скачивается отдельно (до 4 MiB), публикуется с
проверкой SHA-256 и включается в метаданные. Для аудиофайла-документа
сначала проверяются формат, теги и SHA-256 локально, затем серверная копия и её
digest проверяются перед регистрацией. Разрешение `can_be_copied` обязательно;
запрещённый контент не переотправляется через upload. `sha256` в `imports` и
`import_review` допускает `null`, если байты не читались. Старые строковые digest
и журналы продолжают читаться. Локальные импорты отправляются как Telegram audio
с ограниченными display-полями; полные метаданные сохраняются в TrackRegister.

Импорт сохраняет локальный журнал до отправки. `imports` возвращает безопасные
сведения без файловых путей, `resume_import` продолжает по `op_id`.
При `upload_unknown` выполняется поиск MEDIA-marker; если результат остаётся
неизвестным, возвращается `TIMEOUT` без повторной отправки. Если сообщение найдено,
проверяются marker, digest и размер, затем завершается регистрация. Исходный файл
нужен только для ещё не отправленного импорта; его содержимое перепроверяется по
SHA-256. `inspect_orphan_media` показывает MEDIA без действующей ссылки и наличие
локального журнала восстановления; эта операция ничего не удаляет.

CLI использует те же сервисы:

```text
kiyora-cli device
kiyora-cli settings
kiyora-cli command <op-id>
kiyora-cli gc coordinator this
kiyora-cli gc coordinator <device-id> --force-confirmed
kiyora-cli gc abandon <plan-id>
kiyora-cli media cache quota <bytes>
kiyora-cli media imports
kiyora-cli media resume <op-id>
kiyora-cli media orphans
kiyora-cli vault resume <chat-id>
```

Генерация схем: `cargo run -p vault-core --example generate-api-schema` из
`src-tauri`. Тест `vault-core/tests/ipc.rs` проверяет JSON-dispatch, отказ неизвестной
версии до изменения состояния, строгие поля, строковые IDs и совпадение схем.
# Event loss and state refresh

Listen for both `kiyora://backend` and `kiyora://resync-required` before requesting
initial state. The latter carries `{ after_sequence: string | null,
missed_events: string }`. It signals a broadcast overrun; it does not claim that
the missed commands failed. Event sequences are decimal strings; compare them as
`BigInt`, discard duplicates and check the profile/session context.

The version 1 `state_snapshot` backend command accepts `{ session_id }` and returns
the current local projection, its SQLite cursor, context and `event_sequence`.
It works offline. Buffer events while reading it, replace the projection, discard
buffered events at or below `event_sequence`, then apply later invalidations.
The watermark is taken **before** the database read: later notifications may
overlap state already in the snapshot and must be handled idempotently. Refresh
runtime, playback, jobs and download state through their existing queries after
an overrun. A stale session returns `CONFLICT`; obtain the current context before
retrying. No frontend visual changes are required by this additive API.
