**Аудит бэкенда по tmp_files — 22 октября 2026**

Основание: tmp_files/prompt.txt и tmp_files/telegram_music_architecture_ru.md. Их SHA-256 совпадают с src-tauri/spec/backend-task.md и src-tauri/spec/telegram_music_architecture_ru.md соответственно. Проверено текущее рабочее дерево, включая незакоммиченные изменения native-плагина и playback. Это аудит реализации и доступности сценариев, а не подтверждение работы с реальным Telegram.

**Вывод:** большая часть ядра написана, но готовность по исходному заданию не достигнута. Есть блокирующий дефект подключения TDLib, недоступные через публичные интерфейсы функции и незавершённые платформенные/восстановительные сценарии. Статус некоторых пунктов в acceptance-status.md завышен относительно сквозного поведения приложения.

**Проверки, выполненные при аудите**

| Проверка | Результат |
| --- | --- |
| cargo check --workspace --offline | Успешно, Linux |
| cargo clippy --workspace --all-targets --offline -- -D warnings | Успешно, Linux |
| cargo test --workspace --offline | 86 passed, 0 failed, 1 ignored |
| cargo fmt --all -- --check | Ошибка: форматирование текущих media.rs native-плагина, vault-core/backend.rs и src/lib.rs |
| Генерация трёх API JSON schemas во временный каталог | Все совпадают с checked-in версиями |
| kiyora-cli --help | Успешно |
| Реальный Telegram-аккаунт | Не использовался |
| Windows / полное Android-приложение / физическое аудиоустройство | В этом аудите не проверялись |

Прохождение Linux-тестов не проверяет Android-код под cfg(target_os = "android") и не доказывает работоспособность connect_native с действительной авторизацией. Единственный real_tdlib-тест проверяет start/version/close; он пропущен по умолчанию. Приведённые ниже дефекты установлены по путям выполнения кода, если явно не указано другое.

Проверки выполнялись без --locked: исходный Cargo.lock ещё не отражает добавленные в рабочем дереве зависимости uuid/vault-media native-плагина. Cargo временно добавил эти две записи; после проверок восстановлено исходное состояние lockfile. Для будущего --locked gate потребуется согласовать manifest и lockfile.

**Что уже реализовано**

- Cargo workspace, строгий протокол, все 12 обязательных доменных действий, чистый reducer, tombstones и детерминированный порядок.
- SQLite/WAL/FTS, числовые Telegram ID с JSON-представлением строками, транзакционное применение событий и cursor, локальные запросы с pagination.
- Настоящий tdjson FFI/worker, correlation, timeouts, обработка updates, auth-запросы и изоляция профилей с OS secret storage.
- SyncEngine, online guard, reconciliation команд, симулятор нескольких клиентов.
- Полные snapshots с barrier, A/B, digest и отдельным commit; транзакционное восстановление; persisted GC allowlist и возобновление удаления.
- Импорт файлов, точные SHA-256 дубликаты, ranges, streaming/seek/prefetch, полный offline download, partial resume, LRU/pins/leases.
- Desktop playback; sharing выбранного источника; кодек backup и восстановление полного backup в новый пустой vault.
- Application service, Tauri adapters, CLI, unit/integration/property tests.

Отсутствие отдельных crates vault-sharing/vault-cli не является недоработкой: их функции объединены в vault-core/vault-telegram, что исходное задание разрешает.

**1. Блокер: реальный путь подключения TDLib завершается InvalidInput**

Требование: prompt §5, критерий 3. В [backend.rs:481](../crates/vault-core/src/backend.rs#L481) connect_native после setTdlibParameters вызывает client.read с getAuthorizationState. В [client.rs:147](../crates/vault-telegram/src/client.rs#L147) этот метод отсутствует в allowlist; выполняется return Err(TelegramError::InvalidInput). До присваивания s.native выполнение не доходит. Это блокирует как первичное подключение через Backend, так и восстановление сессии тем же путём.

Нужно исправить этот вызов/allowlist и добавить проверку всей цепочки Backend → TdClient → native fixture. Сейчас [real_tdlib.rs:14](../crates/vault-telegram/tests/real_tdlib.rs#L14) использует call, поэтому дефект не обнаруживает. Тесты профилей проверяют отказ secure storage до native open, а service-тесты подключают FakeTelegram непосредственно.

**2. Авторизация не связана с жизненным циклом сервиса корректно**

Требование: prompt §§5, 9, 17. [backend.rs:562](../crates/vault-core/src/backend.rs#L562): каждый auth_status при Ready вызывает activate_account. [backend.rs:555](../crates/vault-core/src/backend.rs#L555) и [backend.rs:861](../crates/vault-core/src/backend.rs#L861) создают новый VaultService, даже когда прежний уже существует. Обычный опрос статуса повторно запускает sync; замена сервиса может отменить связанные downloads/remote streams и оставляет риск перекрытия старого и нового actor до завершения старого.

Нужна идемпотентная активация, завершение предыдущего сервиса перед заменой и обработчик смены authorization state. TdClient.authorization_changes существует ([client.rs:96](../crates/vault-telegram/src/client.rs#L96)), но Backend на него не подписывается: auth.state.changed сейчас публикуется при явных вызовах connect/authenticate/logout. Асинхронные переходы QR/login не обеспечивают самостоятельную активацию Backend без опроса.

**3. Публичный API не позволяет получить состав плейлиста и результат старой команды**

Требование: prompt §17, RFC §18. В [src/lib.rs:50](../src/lib.rs#L50) зарегистрированы backend_library и backend_playlists, но нет getPlaylist/getCommandStatus. [Playlist](../crates/vault-domain/src/model.rs#L180) содержит только название/ID/порядковые поля; entries в ответе отсутствуют. В [VaultRequest](../crates/vault-core/src/api.rs#L215) соответствующих запросов тоже нет.

ProjectionReader.command реализован ([reader.rs:41](../crates/vault-sync/src/reader.rs#L41)), но наружу через Tauri не выведен. После таймаута/перезапуска UI не может штатно запросить результат по op_id. Нет и публичных запросов чтения favorites/shared settings, отдельного каталога альбомов/исполнителей. Мутации favorite/settings есть: их нельзя считать отсутствующими целиком. Требуется закончить именно read API, включая состав и доменный порядок плейлиста.

**4. Назначение и передача роли GC-координатора не доведены до пользовательского API/CLI**

Требование: prompt §12, RFC §10.7. Начальное SharedSettings.gc_device_id равно None ([model.rs:163](../crates/vault-domain/src/model.rs#L163)). Snapshot и GC требуют совпадения с локальным device_id ([engine.rs:645](../crates/vault-sync/src/engine.rs#L645), [gc.rs:92](../crates/vault-sync/src/gc.rs#L92)). CLI не имеет команды установки settings/назначения координатора, хотя snapshot create и gc plan/execute заявлены.

Generic Mutate поддерживает VaultSettingsUpdate, но нет специального сценария выбора текущего устройства, передачи роли и проверки незавершённых GC-планов. Локальный device_id отсутствует в обычном Status/Profile. Нужны доступное назначение/передача роли и тест «новый vault → назначить координатора → snapshot → gc plan» через реальный application API/CLI.

**5. Android playback пока представляет собой незавершённую интеграцию**

Требование: prompt §14, RFC §12.4. В текущем рабочем дереве появился [Rust AndroidMedia](../crates/tauri-plugin-kiyora-native/src/media.rs#L39), который вызывает openMediaSession, preparePlayback, pauseNativePlayback, resumeNativePlayback, updateNativePlayback, stopNativePlayback, closeMediaSession. В [KiyoraNativePlugin.kt](../crates/tauri-plugin-kiyora-native/android/src/main/java/io/github/arkfarin/kiyora/nativebridge/KiyoraNativePlugin.kt) реализованы только storagePaths/getSecret/setSecret.

[AndroidManifest.xml](../crates/tauri-plugin-kiyora-native/android/src/main/AndroidManifest.xml) пуст: foreground media service и его permissions отсутствуют. Нет завершённой реализации media notification, headset controls/audio focus и настройки audio/JNI context. Keystore — отдельная уже реализованная часть; наличие Keystore-тестов не подтверждает playback. Требуются Kotlin/Android-реализация, сборка полного приложения с TDLib и проверка foreground/background воспроизведения.

В Android-only Rust-коде есть и статически видимое несовпадение типов: [media.rs:50](../crates/tauri-plugin-kiyora-native/src/media.rs#L50) передаёт результат anyhow_error() типа tauri::Error в tauri::Error::Anyhow, ожидающий anyhow::Error в используемой Tauri 2.12.5. Этот участок нужно исправить и проверить Android-target сборкой; прошедший Linux clippy его не компилирует.

**6. Импорт не имеет восстановления между upload и track.register**

Требование: prompt §13, RFC §20.3 шаг 3. [import_local_file](../crates/vault-core/src/service.rs#L1216) сначала отправляет MEDIA ([строка 1263](../crates/vault-core/src/service.rs#L1263)), затем вызывает execute(track.register) ([строка 1279](../crates/vault-core/src/service.rs#L1279)). До upload не сохраняется import job, а после него не сохраняется возобновляемая связь import_id → server message_id. Поиска существующего MEDIA по import_id и отдельной диагностики orphaned media в этом пути нет.

При сбое между этими шагами остаётся незарегистрированный файл. Повтор с allow_duplicate=true и тем же op_id снова загружает MEDIA; новый message_id меняет intent и при уже существующей команде приводит к CommandCollision. Нужны persisted import stages, reconciliation upload и повтор регистрации с прежним source. В backup-restore уже есть отдельный поиск существующего MEDIA; обычный импорт его не использует.

**7. Обложки и Telegram metadata реализованы частично**

Требование: prompt §13, RFC §11. [import.rs:131](../crates/vault-media/src/import.rs#L131) извлекает artwork и записывает hash/MIME с source=None. ImportAsset.artwork содержит bytes, но [import_local_file](../crates/vault-core/src/service.rs#L1216) их не сохраняет отдельно и не загружает как thumbnail. [send_document](../crates/vault-telegram/src/transport.rs#L378) использует thumbnail=null и disable_content_type_detection=true.

RemoteFile/RemoteMessage ([transport.rs:15](../crates/vault-telegram/src/transport.rs#L15)) не передают title/performer/duration/cover metadata messageAudio. ImportIncomingAudio ([service.rs:756](../crates/vault-core/src/service.rs#L756)) полностью скачивает файл для анализа. Нет API получения обложки, отдельного thumbnail cache и импорта с использованием доступных Telegram tags без полного download. Встроенная картинка остаётся внутри исходного аудиофайла, но отдельный доступ к ней без загрузки аудио не реализован.

Дубликаты сравниваются только по SHA-256 ([service.rs:1236](../crates/vault-core/src/service.rs#L1236)); эвристика performer/title/duration/size отсутствует. Fingerprint по заданию опционален.

**8. Metadata-backup экспортируется, но не восстанавливает непустую библиотеку**

Требование: prompt §16, RFC §16. [service.rs:1048](../crates/vault-core/src/service.rs#L1048) явно возвращает InvalidInput, если в backup есть tracks и kind != Full. Restore разрешён только в свежий пустой vault или как продолжение уже записанного restore plan ([service.rs:1035](../crates/vault-core/src/service.rs#L1035)). Проверяемого restore в тот же vault с контролем owner/generation нет.

Полный backup восстанавливается; это подтверждают service-тесты. Full export ([backend.rs:392](../crates/vault-core/src/backend.rs#L392)) требует заранее иметь полный проверенный кэш каждого трека, сам недостающие файлы не загружает. Для metadata restore нужны проверка ещё доступных исходных media, явное копирование/перепривязка в новый vault и отдельный безопасный сценарий восстановления в прежний vault. Запрет слепого использования старых IDs обоснован ADR 002, но сам сценарий это не заменяет.

**9. Быстрый cold start не обеспечен на всём пути discovery/attach**

Требование: RFC §§1.2, 8.2, 19. [inspect_vault](../crates/vault-telegram/src/discovery.rs#L67) выполняет scan_history(chat, 0, …) с лимитом миллион записей, чтобы найти INIT. Discovery вызывает это для кандидатов, attach — для выбранного канала. Таким образом, до восстановления из snapshot новая установка всё равно может прочитать всю оставшуюся историю vault, включая старые media/user messages.

Нужны быстрый проверяемый путь к INIT (сохранённая ссылка/поисковые hints с fallback), ограниченный поиск commit и измерение полного onboarding/cold-start, а не только SQLite replay.

**10. Обычный sync повторяет дорогую проверку всей post-barrier истории**

Требование: prompt §10, RFC §20.5. [sync_inner](../crates/vault-sync/src/engine.rs#L271) каждый раз читает/декодирует snapshot, затем читает весь post-barrier tail для audit_receipts ([строка 297](../crates/vault-sync/src/engine.rs#L297)), затем отдельно догоняет cursor. require_online вызывает sync перед мутациями и разрешением remote media.

Это обеспечивает полезную проверку известных удалений, но стоимость каждой операции растёт с хвостом журнала. Отсутствуют измерения end-to-end на 10k событий/10k треков/100k entries и механизм сигналов compaction по числу событий/размеру/возрасту. Нужна оптимизация с сохранением обнаружения изменений, а не простое удаление проверки receipts. Имеющийся benchmark 10k SQLite creates не покрывает TDLib requests и эти повторные чтения.

**11. Recovery/repair не покрывает все предусмотренные аварии**

Требование: prompt §11, RFC C05/C15/G06 и §15.5. [latest_commit](../crates/vault-sync/src/engine.rs#L238) возвращает первый распознанный commit от конца; [sync_inner](../crates/vault-sync/src/engine.rs#L271) пробует его A/B и при неудаче останавливается. Проверяемого выбора более старого валидного поколения с доказанно достаточным журналом или восстановления по полному оставшемуся журналу нет. Это безопасный отказ, но более узкая реализация recovery, чем в RFC; слепой fallback на старый snapshot после GC был бы ошибкой.

Нет самостоятельного repair повреждённой SQLite: [switch_profile](../crates/vault-core/src/backend.rs#L127) должен успешно открыть существующую БД до запуска сервиса. Нет сценария «карантин повреждённой БД → восстановление во временную → atomic swap». Восстановление одной A/B-копии из другой не выделено в repair-операцию; новое полноценное поколение можно создать обычным snapshot после назначения координатора.

**12. Нет модели недоступности отдельного media source**

Требование: prompt §14, RFC C17/E12. [Source](../crates/vault-domain/src/model.rs#L85) не содержит availability; локальной таблицы/DTO доступности тоже нет. В [media_file](../crates/vault-sync/src/engine.rs#L585) отсутствие remote message/file превращается в общий Integrity. В [DownloadState](../crates/vault-core/src/api.rs#L289) отсутствует UNAVAILABLE.

Track metadata при этом не удаляется — это правильно. Не реализованы статус проблемного источника, различимая MEDIA_SOURCE_MISSING, подсчёт missingMediaSources и выбор другого рабочего source. SourceRelink как доменная операция есть; диагностика и пользовательский процесс relink не завершены.

**13. Локальные операции кэша зависят от наличия сетевого VaultService**

Требование: prompt §9, RFC §7.2. [Backend.service_request](../crates/vault-core/src/backend.rs#L894) требует s.service даже для GetCacheStats/ClearLocalCache/PinCachedMedia. После disconnect сервис удалён, а после выбора профиля до авторизации он ещё не создан. При этом CacheManager и локальная проекция могут быть доступны. В таком состоянии очистка кэша возвращает NoVault, хотя она должна работать локально.

Нужно обслуживать локальный cache API независимо от online service. Квота также зафиксирована в 10 GiB ([backend.rs:143](../crates/vault-core/src/backend.rs#L143)); публичной настройки/сохранения квоты нет. Отдельный каталог TDLib files ([vault-security](../crates/vault-security/src/lib.rs#L178)) не входит в учёт CacheManager; в адаптере нет интеграции deleteFile/optimizeStorage. Следует определить общую политику дискового пространства двух хранилищ, иначе заявленная квота не ограничивает их суммарный размер.

**14. Progress/events/диагностика не завершены**

Требование: prompt §17, RFC §§18, 21. Типы событий есть, но [refresh](../crates/vault-core/src/service.rs#L618) после удалённого replay публикует общий library.changed. playlist.changed публикуется только в обработчике локального Mutate ([service.rs:680](../crates/vault-core/src/service.rs#L680)); найденный при reconnect итог ранее неизвестной команды отдельно через command.result не публикуется.

Sync progress — boolean, snapshot/GC progress — преимущественно start/end. Нет прогресса по страницам истории/стадиям проверки/батчам GC и upload/import job. Tauri создаёт новые CancellationToken на вызов, но не отдаёт handle для отмены этих долгих операций; cancelDownload — реализованное исключение.

Verify возвращает SnapshotInfo ([service.rs:688](../crates/vault-core/src/service.rs#L688)), а не полную VaultDiagnostics: нет missing sources, количества неизвестных записей, возраста snapshot, размера DB, непокрытого журнала и истории GC. Ошибки ownership/autodelete/protocol часто сводятся к общему Integrity ([api.rs:77](../crates/vault-core/src/api.rs#L77)). CLI diagnose ([kiyora-cli.rs:921](../crates/vault-core/src/bin/kiyora-cli.rs#L921)) в основном печатает профили/настройку пути tdjson; полноценного безопасного диагностического отчёта и структурированного логирования нет.

**15. CLI отображает неправильный порядок элементов плейлиста**

Требование: prompt §§7, 18. [playlist show](../crates/vault-core/src/bin/kiyora-cli.rs#L736) сортирует entries только по order_key. Доменный порядок вычисляется обходом after_entry_id через [State.playlist_order](../crates/vault-domain/src/reducer.rs#L390). Эти порядки различаются после вставок/перемещений.

Пример: A(root, order=1), B(root, order=2), C(after=A, order=3). Доменный порядок A,C,B; текущая CLI покажет A,B,C. Нужно использовать общий метод и покрыть сценарий CLI-тестом. Дополнительно отсутствуют CLI simulate, статус команды по op_id, установка vault settings/GC role, редактирование metadata и resume прерванного создания vault. Отдельные pause/seek CLI-процессы не управляют уже запущенным плеером; работающие controls доступны внутри playback play, что документация оговаривает.

**16. Bulk-import, Android file access и очередь проигрывания отсутствуют как законченные сценарии**

Требование: prompt §§13, 17, RFC E14, §§8.4, 12.4. Публичный Import принимает один filesystem path и выполняется в общем actor. Нет ImportJob для списка файлов, per-file checkpoint/progress/cancel и управляемой очереди импорта. В Android-плагине нет ContentResolver/SAF-адаптера, который переносит content URI выбранного аудио в доступный Rust файл.

Playback поддерживает отдельный track и его controls. Очередь плейлиста, next/previous и политика обновления очереди при remote edit в backend не представлены. Это отдельная недостающая функциональность; сам streaming/seek уже реализован. MPRIS/Windows media session также отсутствуют, но RFC относит их к реализации по возможностям платформы.

**17. Контракт и regression suite покрывают не весь application boundary**

Требование: prompt §§17, 19–21. [generate-api-schema.rs](../crates/vault-core/examples/generate-api-schema.rs#L9) описывает только VaultRequest + PlaybackAction для запросов и VaultResponse + ApiError для ответов. Полный набор Tauri-команд, аргументы auth/profiles/discovery/library/export и соответствующие ответы не покрыты единым генерируемым контрактом.

Симулятор и property tests полезны и проходят, но нет полной трассировки RFC A01–G10 на проверяемые сценарии. [convergence.rs](../crates/vault-sync/tests/convergence.rs) содержит 9 интеграционных сценариев, а не систематический crash после каждого сетевого шага. Нет test-account integration suite для login → vault → import → two-client sync → restore/share/GC: [real_tdlib.rs](../crates/vault-telegram/tests/real_tdlib.rs) содержит только credential-free lifecycle smoke. Это отсутствие тестовой реализации, а не просто отсутствие credentials для запуска готовой suite.

**18. CI, упаковка и платформенная приёмка не закончены**

Требование: prompt §21, RFC §19 фаза 6. В репозитории нет .github/workflows или другого CI pipeline. Есть локальные check-backend.sh/check-android-plugin.sh, но нет автоматической матрицы Linux/Windows/Android, release artifacts и полной проверки соответствия протокола в CI.

Windows TDLib/app build и complete Android app/TDLib не подтверждены. [android.md](android.md) отдельно указывает необходимость Android NDK, Tauri Android project, упаковки libtdjson.so и native dependencies. Эти сборки не эквивалентны проверке Rust/Kotlin native-плагина. Audible desktop output, streaming с реального аккаунта и длительный Android playback также остаются непроверенными. Корневой README остаётся шаблоном Tauri; инструкции бэкенда разбросаны по docs/spec и не образуют проверенный end-to-end путь нового пользователя.

**Границы аудита и разумный порядок следующей работы**

Frontend src/App.tsx остаётся шаблоном; это соответствует явному требованию prompt «FRONTEND НЕ ТРОГАТЬ». Его будущая интеграция — отдельная задача. Обязательного удалённого HTTP-бэкенда по заданию нет, и его отсутствие не является пробелом. WebRTC P2P, автоматический media GC, внешний metadata catalog, encrypted offsite backup и совместное редактирование не нужно включать в блокеры MVP.

Рекомендуемый порядок:

1. Исправить connect_native и идемпотентность auth/service lifecycle, проверить их через Backend и native fixture.
2. Закончить getPlaylist/getCommandStatus/read settings/favorites и управление GC-координатором; поправить CLI order.
3. Реализовать persisted import/reconciliation, metadata restore и независимые локальные cache operations.
4. Завершить обложки, source availability, диагностику, события/progress/cancel и полный API-контракт.
5. Закончить Android media/file adapters и упаковку; добавить CI и тестовую suite с явным opt-in для отдельного Telegram-аккаунта.
6. Измерить cold start/live sync на больших библиотеках, расширить chaos matrix; обновить acceptance-status по фактическим результатам.

В ходе аудита исходный код не изменялся; добавлен только этот отчёт. Результаты cargo-проверок относятся к рабочему дереву, увиденному 2026-10-22, и не подтверждают последующие изменения.
