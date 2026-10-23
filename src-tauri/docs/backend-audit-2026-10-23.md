**Аудит текущего бэкенда относительно tmp_files — 23 октября 2026**

**Вывод: архитектура и значительная часть реализации соответствуют заданию, но весь backend ещё не завершён.** Остаются недостающие пользовательские сценарии, восстановление после повреждений, интеграция медиа и платформенная приёмка. Кроме того, текущая автоматическая проверка не проходит из-за рассогласования JSON Schema с Rust DTO.

Проверено рабочее дерево с незакоммиченными изменениями, а не только HEAD. Источники требований: [prompt.txt](/home/arkfarin/Projects/kiyora/tmp_files/prompt.txt) и [архитектура](/home/arkfarin/Projects/kiyora/tmp_files/telegram_music_architecture_ru.md). Их содержимое побайтно совпадает с `src-tauri/spec/backend-task.md` и `src-tauri/spec/telegram_music_architecture_ru.md`; расхождения вызваны реализацией, а не разными версиями задания.

Область проверки: Rust workspace в src-tauri, SQLite/migrations, протокол, TDLib, security/profiles, sync/snapshot/GC, media/cache/playback, application service, IPC, CLI, Android Rust/Kotlin, тесты, схемы и сборочные скрипты. В корневых crates/spec нет отдельной действующей реализации бэкенда. Личная `tmp_files/telegram.session` не открывалась и не использовалась. Исходный код, схемы и существующие отчёты при аудите не исправлялись; добавлен только этот документ.

**Результаты заново выполненных проверок**

| Проверка | Фактический результат |
| --- | --- |
| `CARGO_NET_OFFLINE=true bash src-tauri/scripts/check-backend.sh` | fmt, workspace check и Clippy с `-D warnings` прошли; скрипт остановился на тесте IPC/schema |
| `cargo test --locked --offline --workspace --no-fail-fast` | **105 passed, 1 failed, 1 ignored**; единственный failed — `checked_in_schemas_match_the_runtime_boundary` |
| Генератор всех трёх API schemas во временный каталог | request совпадает; response и event отличаются определением `ImportStage`: отсутствует `staging` |
| `kiyora-cli --help` | Успешно; CLI-тесты также прошли в workspace |
| Настоящая tdjson 1.8.69: `real_native_start_version_close` | Отдельный opt-in запуск прошёл; **без авторизации аккаунта** |
| `scripts/check-android-rust.sh` | Успешная сборка/линковка текущего Rust app в arm64 `.so`; AAudio, JNI exports и 16 KiB LOAD alignment проверены |
| `scripts/check-android-plugin.sh` | Сборка плагина, instrumentation APK и Android Lint прошли |
| Android instrumentation на устройстве | В этом аудите не запускалась. Существующие логи от 22 октября подтверждают 12 тестов и 5 повторов двух lifecycle-сценариев; это предыдущие результаты |
| Полное Android APK с TDLib / Windows / физическое аудио | В этом аудите не проверены |
| Авторизация, upload/download/share/GC на реальном Telegram-аккаунте | Не выполнялись; готовой сквозной account-suite в репозитории нет |

Android использовал существующие NDK r28c, Rust 1.99.1 из `target/android-tools/rust-sysroot`, SDK и Gradle 8.14.3. Локальные логи этого аудита: `/tmp/kiyora-backend-audit-{check,tests,android-rust,android-plugin}-2026-10-23.log`. Сгенерированные схемы: `/tmp/kiyora-backend-audit-schema-2026-10-23/`.

**Что уже реализовано и не следует повторно записывать в отсутствующие функции**

| Подсистема | Подтверждённая реализация |
| --- | --- |
| Domain/protocol | Все 12 обязательных действий, UUID, числовой server order, idempotency, tombstones/delete-wins, строгие форматы и compatibility gates |
| SQLite | WAL/FK, миграция v1, FTS/pagination, атомарные reducer + receipts + cursor, транзакционное восстановление проекции |
| Telegram/security | Настоящий tdjson worker/FFI, correlation, auth, bounded queues, cancellation/timeouts, allowlist безопасных повторов, OS secret storage и блокировка профиля между процессами |
| Sync | History/catch-up/live updates, unknown ACK reconciliation, запрет общих мутаций вне ONLINE_READY, симуляция нескольких клиентов |
| Snapshots/GC | Полный snapshot на barrier, A/B и независимая проверка, отдельный commit, сохранённый allowlist GC, подтверждение digest, проверки между батчами, resume после сбоя |
| Read API | PlaylistDetail с доменным порядком, command status, favorites/settings/device, albums/artists/imports; доступны через Query/IPC |
| GC role | Назначение текущего устройства, передача роли с expected previous, явный forced takeover, abandon планов и отказ устаревшему координатору |
| Media | Теги и SHA-256, точные дубликаты, ranges/seek/prefetch, PCM worker, полный download и partial resume, LRU/quota/pin/leases |
| Импорт | Журнал стадий, MEDIA marker, reconciliation после lost ACK, диагностика orphan media, **постоянная подготовленная копия файла**, resume без исходного файла, очистка после подтверждённой регистрации |
| Local cache | Работа без авторизации/сетевого сервиса, настройка и сохранение квоты |
| Sharing | Проверка выбранного зарегистрированного источника и получателя, copy/forward, явный incoming import; проверено симулятором |
| IPC/CLI | Общие application services, versioned BackendRequest/Response, контекст профиля/сессии и sequence событий, CLI без frontend |
| Android | Keystore, JNI/media bridge, MediaSession, foreground service, notification, focus, wake lock, fencing устаревших команд; текущая сборка прошла |

Примеры проверенных путей: [SQLite transaction](/home/arkfarin/Projects/kiyora/src-tauri/crates/vault-db/src/lib.rs:174), [snapshot publish](/home/arkfarin/Projects/kiyora/src-tauri/crates/vault-snapshot/src/lib.rs:265), [GC batches](/home/arkfarin/Projects/kiyora/src-tauri/crates/vault-sync/src/gc.rs:255), [auth watcher](/home/arkfarin/Projects/kiyora/src-tauri/crates/vault-core/src/backend.rs:620), [import resume](/home/arkfarin/Projects/kiyora/src-tauri/crates/vault-core/src/imports.rs:272), [durable import storage](/home/arkfarin/Projects/kiyora/src-tauri/crates/vault-core/src/imports/storage.rs:77).

**Незавершённые работы, в порядке практической важности**

1. **Согласовать схемы с исполняемым API и вернуть зелёную проверку.**

   [ImportStage](/home/arkfarin/Projects/kiyora/src-tauri/crates/vault-core/src/imports.rs:20) уже содержит `Staging`, сериализуемый как `staging`. В [response schema](/home/arkfarin/Projects/kiyora/src-tauri/spec/schema/backend-response-v1.schema.json:1356) и [event schema](/home/arkfarin/Projects/kiyora/src-tauri/spec/schema/backend-event-v1.schema.json:1197) этого значения нет. Из-за этого падает [проверка схем](/home/arkfarin/Projects/kiyora/src-tauri/crates/vault-core/tests/ipc.rs:65). Это конкретный текущий дефект контракта, а не отсутствие IPC вообще.

   Завершение: перегенерировать обе схемы штатным генератором, выполнить check-backend и обновить документы с результатами. В этом аудите схемы намеренно не исправлялись.

2. **Закончить metadata backup restore и восстановление в прежний vault.**

   [ImportBackup](/home/arkfarin/Projects/kiyora/src-tauri/crates/vault-core/src/service.rs:1112) требует свежую пустую библиотеку; при наличии tracks и режиме не Full явно возвращает InvalidInput на строках 1126–1129. Следовательно, metadata-backup обычной непустой библиотеки можно экспортировать/проверить, но нельзя штатно восстановить. Full backup в новый пустой vault восстанавливается и покрыт тестом.

   Нет отдельного безопасного сценария восстановления в тот же vault с проверкой owner/generation и явным разрешением конфликтов. [Full export](/home/arkfarin/Projects/kiyora/src-tauri/crates/vault-core/src/backend.rs:485) дополнительно требует заранее закэшировать каждый трек: недостающие media он сам не скачивает, а возвращает InvalidInput.

   Завершение: metadata restore с проверкой доступности исходных media и корректным переносом/перепривязкой; отдельный план восстановления прежнего vault; понятный preflight полного экспорта с перечнем недостающих файлов и управляемой загрузкой. Требования: prompt §16, RFC §16.

3. **Добавить ремонт повреждённой SQLite и законченный recovery workflow.**

   [switch_profile](/home/arkfarin/Projects/kiyora/src-tauri/crates/vault-core/src/backend.rs:157) требует успешно открыть существующую SQLite до запуска сервиса. Повреждённый файл останавливает этот путь. Отдельной операции «сохранить повреждённую копию → восстановить временную БД из Telegram → проверить → атомарно заменить» нет. Транзакционный restore в уже открытой исправной БД не заменяет этот сценарий.

   [latest_commit](/home/arkfarin/Projects/kiyora/src-tauri/crates/vault-sync/src/engine.rs:243) выбирает первый распознанный commit от конца, затем [sync_inner](/home/arkfarin/Projects/kiyora/src-tauri/crates/vault-sync/src/engine.rs:281) проверяет только его A/B. Если обе копии непригодны, выполняется безопасный отказ, но нет поиска более раннего проверенного поколения с доказанно достаточным журналом или восстановления по ещё целому журналу. Автоматически использовать старый snapshot после pruning нельзя. Восстановление недостающей A/B-копии не выделено в repair-сценарий; обычное создание нового поколения уже возможно.

   Завершение: публичный repair API/CLI, сохранение повреждённых данных для диагностики, crash-тесты записи/замены БД, проверяемые варианты fallback и явные причины невозможности восстановления. Требования: RFC C05/C15/G06, §§9.4, 15.5.

4. **Реализовать доступность отдельных источников и понятный relink.**

   [media_file](/home/arkfarin/Projects/kiyora/src-tauri/crates/vault-sync/src/engine.rs:611) превращает отсутствие сообщения/файла в общий Integrity. В [DownloadState](/home/arkfarin/Projects/kiyora/src-tauri/crates/vault-core/src/api.rs:386) отсутствует Unavailable, нет локальной модели здоровья source и диагностики missingMediaSources. Метаданные трека сохраняются — это правильно. Доменное действие SourceRelink уже существует; не завершены обнаружение проблемы, состояние API и пользовательский сценарий выбора другого источника.

   Завершение: различать missing/temporary/permanent ошибки источника, сохранять availability локально, выдавать точную диагностику, выбирать/перепривязывать рабочий source с online guard. Требования: prompt §14, RFC C17/E12.

5. **Довести обложки, Telegram metadata и эвристические дубликаты.**

   [Парсер локального файла](/home/arkfarin/Projects/kiyora/src-tauri/crates/vault-media/src/import.rs:128) извлекает artwork bytes/hash, но сохраняет metadata.artwork.source=None. [Импорт](/home/arkfarin/Projects/kiyora/src-tauri/crates/vault-core/src/imports.rs:246) не публикует отдельную обложку и не создаёт её доступный источник. [Отправка](/home/arkfarin/Projects/kiyora/src-tauri/crates/vault-telegram/src/transport.rs:380) использует document с thumbnail=null.

   [RemoteFile/RemoteMessage](/home/arkfarin/Projects/kiyora/src-tauri/crates/vault-telegram/src/transport.rs:14) не сохраняют title/performer/duration/album-cover из messageAudio. [Incoming import](/home/arkfarin/Projects/kiyora/src-tauri/crates/vault-core/src/service.rs:832) сначала скачивает весь файл. Отдельного cover API/cache нет; вероятные дубликаты по metadata/duration/size не вычисляются, есть только SHA-256. Fingerprint опционален и не является обязательным пробелом.

   Возможности metadata/thumbnail сверены с [официальной схемой закреплённой TDLib](https://github.com/tdlib/td/blob/9d92b49bc5d0d2e29fe518239af647a88f8680e6/td/generate/scheme/td_api.tl): типы audio и inputAudio передают эти поля. Это ограничение текущего адаптера, а не отсутствие таких полей в выбранной версии TDLib.

   Завершение: передача и хранение обложки с самостоятельной ссылкой, thumbnail cache/API, сохранение TDLib audio metadata, получение доступных тегов без полного download, review вероятных дубликатов без auto-merge. Требования: prompt §13, RFC §11.

6. **Закончить массовый импорт и доступ к Android-файлам.**

   [VaultRequest::Import](/home/arkfarin/Projects/kiyora/src-tauri/crates/vault-core/src/api.rs:310) принимает один filesystem path. Нет API задания со списком файлов, ограниченной конкурентностью, per-file результатами и отменой/возобновлением всего задания. Долгий import исполняется внутри [общего service actor](/home/arkfarin/Projects/kiyora/src-tauri/crates/vault-core/src/service.rs:453), задерживая обработку других команд и updates. В Android native слое отсутствует ContentResolver/SAF-адаптер выбранного content URI в поток/доступный Rust файл.

   Постоянный staging одиночного файла уже добавлен и проверен, включая исчезновение исходника и crash между публикацией копии и checkpoint. Не нужно реализовывать его повторно. Остаются bulk lifecycle, отмена/разбор зависших заданий и интеграция Android file access. Требования: prompt §17 importLocalFiles, RFC E14 и §1.1.

7. **Обеспечить быстрый onboarding и измерить стоимость sync.**

   [inspect_vault](/home/arkfarin/Projects/kiyora/src-tauri/crates/vault-telegram/src/discovery.rs:68) сканирует историю от начала с пределом 1 000 000 сообщений для поиска INIT. Discovery проверяет таким способом подходящие каналы; attach также использует inspect_vault. Поэтому onboarding новой установки всё ещё может читать многолетнюю историю, даже при наличии snapshot. Обычная активация уже сохранённой привязки запускает сервис непосредственно и этого discovery-сканирования не требует.

   [sync_inner](/home/arkfarin/Projects/kiyora/src-tauri/crates/vault-sync/src/engine.rs:276) повторно восстанавливает/декодирует snapshot, читает весь post-barrier tail для audit_receipts и затем отдельно catch-up. Этот путь вызывается и при online guard перед действиями. Проверка receipts полезна для обнаружения известных удалений; удалять её без эквивалентной проверки нельзя. GC дополнительно повторяет широкую проверку истории на границах батчей.

   Завершение: проверяемые INIT/commit hints с fallback, отсутствие полного исторического scan в нормальном onboarding, измерения реального числа запросов/памяти/времени; оптимизация с сохранением проверок целостности; сигналы compaction по размеру/числу событий/возрасту. Имеющийся 1k sync-тест и 10k локальный SQLite benchmark не подтверждают RFC-нагрузку 10k tracks, 100k entries, 10k tail events. Требования: RFC §§1.2, 8.2, 9.7, 20.5.

8. **Закончить события, прогресс и отмену долгих операций.**

   [refresh](/home/arkfarin/Projects/kiyora/src-tauri/crates/vault-core/src/service.rs:631) после remote replay публикует library.changed. playlist.changed и command.result публикуются в обработчиках локальных действий, но не обеспечен такой же точный поток для удалённых изменений и найденного после reconnect результата ранее неизвестной команды.

   [EventData](/home/arkfarin/Projects/kiyora/src-tauri/crates/vault-core/src/events.rs:11) имеет нужные имена и контекст, однако sync.progress — running bool; snapshot — start/end; GC не стримит прогресс каждого батча. Import stage events и download bytes уже есть. [Tauri invoke](/home/arkfarin/Projects/kiyora/src-tauri/src/lib.rs:101) создаёт новый CancellationToken, не предоставляя вызывающему общий job handle/cancel. cancelDownload — реализованное исключение.

   Завершение: единый lifecycle долгих заданий, адресная отмена, stage/page/batch progress, события по результатам replay и reconnect. Требования: prompt §§17,20, RFC §§8.4,18.

9. **Добавить полноценную диагностику.**

   [Verify](/home/arkfarin/Projects/kiyora/src-tauri/crates/vault-core/src/service.rs:717) возвращает SnapshotInfo, а не предусмотренный VaultDiagnostics. Не хватает состояния missing sources/unknown records, возраста snapshot, размера БД, непокрытого журнала и истории GC. [ApiError](/home/arkfarin/Projects/kiyora/src-tauri/crates/vault-core/src/api.rs:77) сводит разные ownership/protocol/snapshot проблемы к общему Integrity. [CLI diagnose](/home/arkfarin/Projects/kiyora/src-tauri/crates/vault-core/src/bin/kiyora-cli.rs:1051) выводит главным образом число профилей и наличие настройки tdjson. Систематического структурированного tracing по операциям не найдено.

   Завершение: типизированный диагностический отчёт, различимые actionable error codes и безопасные локальные логи без секретов/сырых приватных сообщений. Требования: RFC §§18.2,21.

10. **Согласовать дисковую квоту с хранилищем TDLib.**

    Пользовательская квота CacheManager уже есть. Однако TDLib получает отдельный [files_directory](/home/arkfarin/Projects/kiyora/src-tauri/crates/vault-core/src/backend.rs:578), не учитываемый CacheManager. Интеграции deleteFile/optimizeStorage в адаптере нет. Подготовленные импорты также имеют отдельный лимит, но не входят в обычный cache stats.

    Завершение: показать суммарное и раздельное использование диска, определить очистку TDLib files с учётом активных загрузок/декодеров, явно отразить квоты staging и verified cache. Сейчас значение «квота кэша» не ограничивает весь объём файлов приложения. Требования: prompt §14, RFC §§12.3,21.

11. **Реализовать очередь проигрывания и довести CLI.**

    Playback умеет один track, pause/resume/stop/seek/volume; [PlaybackAction](/home/arkfarin/Projects/kiyora/src-tauri/crates/vault-core/src/api.rs:269) не содержит очереди/next/previous. Нет политики снимка очереди при удалённых изменениях плейлиста, предусмотренной RFC §8.4.

    В [CLI dispatch/help](/home/arkfarin/Projects/kiyora/src-tauri/crates/vault-core/src/bin/kiyora-cli.rs:47) нет simulate и команды редактирования metadata. Каждый запуск создаёт свой Backend; отдельный `playback pause` не управляет предыдущим процессом. Рабочие controls есть внутри интерактивной playback_session. Ошибка доменного порядка `playlist show`, отмеченная 22 октября, уже исправлена и покрыта тестом.

    Завершение: очередь и её стабильность при remote edit, CLI simulate/metadata edit, согласованный способ управления длительной playback-сессией; полноценная diagnose относится к пункту 9. MPRIS/Windows media session предусмотрены RFC «по возможностям» и не равнозначны отсутствию самого playback.

12. **Закончить платформенную поставку и сквозную приёмку.**

    Android bridge/service теперь реализованы и текущая компиляция подтверждена. Отдельная `.so` и тестовый native APK не заменяют полный Tauri APK с libtdjson.so/native dependencies, реальный запуск, login, streaming и PCM через JNI. Нужны foreground/background/notification/headset и длительный screen-off playback на реальном устройстве. На desktop также не подтверждён слышимый звук на физическом выходе. Windows build/TDLib packaging не проверены.

    [tauri.conf.json](/home/arkfarin/Projects/kiyora/src-tauri/tauri.conf.json) не задаёт готовую поставку TDLib для всех платформ; [android.md](/home/arkfarin/Projects/kiyora/src-tauri/docs/android.md) прямо описывает ещё необходимые packaging-шаги. CI pipeline отсутствует. Корневой README остаётся шаблонным, хотя отдельные API/Android/TDLib инструкции уже написаны.

    Завершение: воспроизводимые Linux/Windows/Android сборки и native artifacts, CI, проверенная установка и единая инструкция от профиля до backup/recovery. Требования: prompt §21 и критерии 1/12/20, RFC фаза 6.

13. **Завершить тестовую матрицу и написать account-suite.**

    Симулятор, property/integration tests и native ABI fixture существуют и полезны. Они не закрывают автоматически весь каталог A01–G10 и сбои после каждого сетевого шага. В частности, отсутствующие сценарии metadata restore/repair/availability/bulk/queue пока не могут иметь полную приёмку. Нагрузочные и аппаратные сценарии также остаются.

    [real_tdlib.rs](/home/arkfarin/Projects/kiyora/src-tauri/crates/vault-telegram/tests/real_tdlib.rs:5) содержит только start/version/authorization-state/close без аккаунта. Поэтому проблема состоит не только в отсутствии предоставленных credentials: нужно ещё написать opt-in suite для login/session reopen → vault → import → два клиента → recovery/backup/share/GC. Запуск с account mutations требует отдельной явно заданной тестовой конфигурации; личная session из tmp_files её не заменяет.

    Завершение: трассировка обязательных RFC-сценариев на конкретные тесты, fault injection на границах commit/upload/GC, нагрузочные проверки и отдельный безопасный account test runner. Требования: prompt §19, RFC §20.

**Оценка 20 критериев готовности из prompt §22**

«Реализовано, локально проверено» означает достаточное локальное доказательство указанной функции, но не подтверждение работы с живым аккаунтом на всех платформах.

| № | Критерий | Текущая оценка |
| --- | --- | --- |
| 1 | Workspace/целевые сборки | Linux и Android arm64 library прошли; Windows и полный APK не подтверждены |
| 2 | Настоящий TDLib adapter | Реализовано; настоящий native lifecycle прошёл |
| 3 | Авторизация/сессия | Реализовано и проверено native fixture; login/reopen реального аккаунта не подтверждены |
| 4 | Discovery/create vault | Реализовано, симулятор; остаются быстрый discovery и live приёмка |
| 5 | SQLite/миграции | Реализовано, локально проверено; repair повреждённой БД отдельно не завершён |
| 6 | Обязательные reducer events | Реализовано, локально проверено |
| 7 | ONLINE_READY guard | Реализовано, локально проверено |
| 8 | History/live sync | Реализовано, симулятор; масштабирование/live приёмка остаются |
| 9 | Snapshot recovery | Основной путь и A→B есть; repair и безопасные дополнительные recovery-пути не завершены |
| 10 | Проверяемый safe GC | Реализовано, симулятор; полная chaos/live приёмка остаётся |
| 11 | Локальный импорт | Основной путь и durable resume есть; covers/heuristics/bulk/Android file access не завершены |
| 12 | Download/cache/real playback | Существенная реализация и decoder-тесты; availability/queue/TDLib quota/full app/hardware остаются |
| 13 | Sharing | Реализовано, симулятор; live forwarding не подтверждён |
| 14 | Backup/export/import | Частично: metadata restore и same-vault recovery отсутствуют |
| 15 | CLI | Работает, но simulate/metadata edit/diagnose и управление playback между процессами не завершены |
| 16 | Типизированный Tauri API | Реализован; две опубликованные схемы устарели, jobs/diagnostics/bulk неполны |
| 17 | Все доступные проверки проходят | **Нет: schema drift test падает** |
| 18 | Frontend не изменён | В рабочем дереве frontend не изменён; аудит его не менял |
| 19 | Реальный Telegram smoke при безопасной конфигурации | Account smoke не выполнялся; есть только credential-free native smoke |
| 20 | Честно перечислены непроверенные платформы | Ограничения документированы, но сводные результаты тестов требуют актуализации |

**Как изменился вывод по сравнению с аудитом 22 октября**

Старые пункты про InvalidInput в connect_native, отсутствие auth watcher/идемпотентной активации, read API, GC role, Kotlin media service, зависимость local cache от сети и неправильный порядок CLI больше не описывают текущий код. Эти изменения подтверждены чтением путей исполнения и текущими тестами/сборкой.

Кроме того, remediation-status всё ещё считает durable pre-upload staging открытым, хотя в текущем дереве он уже реализован и соответствующие тесты прошли. В обратную сторону, документы сообщают о 98 прошедших тестах и совпадении схем, тогда как сейчас получено 105 passed / 1 failed / 1 ignored и две несовпадающие схемы. Старые отчёты следует воспринимать как исторические снимки.

**Практический порядок завершения**

1. Согласовать schemas и документы; получить полностью зелёный локальный набор проверок.
2. Закрыть recovery/metadata restore/source availability: это сценарии, при которых пользователь уже сейчас не сможет восстановить или обслужить свою библиотеку штатным API.
3. Закончить metadata/covers, bulk import/SAF, очередь и общий job/progress/cancel/diagnostics API; учесть TDLib disk usage.
4. Измерить и исправить onboarding/sync/GC на целевых объёмах, сохранив проверки целостности.
5. Добавить CI/упаковку, account-suite и выполнить платформенную приёмку; обновить матрицу готовности по результатам.

Процент готовности здесь не назначается: объём кода и число тестов не дают надёжной оценки оставшегося риска. Текущий статус — **развитое работающее ядро с частично завершёнными прикладными сценариями, ещё не backend, полностью готовый по tmp_files**. Отсутствие отдельного HTTP-сервера, crates vault-cli/vault-sharing и готового React-интерфейса не является нарушением: сервер не требуется, небольшие crates разрешено объединять, frontend исходным заданием запрещено менять. WebRTC, автоматический media GC и другие явно отложенные функции не включены в обязательный остаток.
