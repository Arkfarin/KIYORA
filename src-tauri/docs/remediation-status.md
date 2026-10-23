**Работы по аудиту 2026-10-22**

Цель: исправить все 18 пунктов backend-audit-2026-10-22.md и закрыть требования tmp_files, сохранив запрет на изменение frontend. Исходный аудит остаётся снимком состояния до исправлений; результаты работ фиксируются здесь.

| Пункты | Работа | Статус / доказательство |
| --- | --- | --- |
| 1–2 | Подключение TDLib, auth updates, идемпотентный lifecycle | Исправлены allowlist, асинхронная активация, retry getMe, повторная активация и контекст событий. Native ABI fixture проверяет весь путь Backend без аккаунта, включая transient getMe и bound vault; live account acceptance остаётся открытой. |
| 3 | Полный локальный read API | Реализованы PlaylistDetail с доменным порядком, command status, favorites/settings/device, albums/artists и imports. `backend_query` и versioned `backend_invoke`; тесты queries, profiles, IPC проходят. |
| 4 | Назначение и передача GC-роли | Реализованы API/CLI, выбор текущего устройства, expected previous, явный forced takeover, запрет обхода через generic settings, abandon и отказ старых планов. Engine и application-service тесты проходят. Роль не является серверным lock; пределы описаны в api.md. |
| 5 | Android playback/service/JNI | Реализованы Rust/Kotlin bridge, foreground media service, MediaSession, уведомление, audio focus, wake lock и защита от устаревших команд. Полный Rust app проверен для arm64; Android instrumentation проверяет OS service/focus/notifications/background resume и отказ при пропавших heartbeat. JNI context берётся из locked Tao. Полный APK с TDLib, сквозной PCM через Tauri и физическое аудио ещё требуют приёмки. |
| 6 | Возобновляемый импорт, orphan reconciliation | Реализован persisted journal до отправки, resume по op_id, reconciliation MEDIA marker, диагностика orphan media и события import.progress. Тесты проверяют lost ACK → restart → регистрация без исходного файла/повторного upload и отказ слепого повтора при неизвестном результате без marker. Продолжение ещё не отправленного импорта требует доступного исходного файла; durable staging/bulk lifecycle остаются открытыми. |
| 7 | Обложки, Telegram metadata, duplicate review | Ожидает |
| 8 | Metadata/full backup restore/export | Ожидает |
| 9–10 | Discovery и масштабирование sync | Ожидает |
| 11 | SQLite/snapshot repair | Ожидает |
| 12 | Source availability/relink | Ожидает |
| 13 | Независимый cache API, квоты, TDLib storage | Локальные cache calls и persisted quota работают без сервиса/авторизации; тестируются pin/lease/reservation и отказ secure storage при восстановлении профиля. Учёт/очистка отдельного TDLib files storage ещё не реализованы. |
| 14 | События/progress/cancel/диагностика | Добавлены auth lifecycle и import stage events, orphan media query; remote replay events, детальный sync/snapshot/GC progress, универсальная отмена и полная диагностика остаются. |
| 15 | CLI | Исправлен playlist show с регрессионным CLI-тестом A,C,B; добавлены command/device/settings, GC role/abandon, quota, import resume/orphans, vault resume. Исправлено повторное взятие session mutex в resume creation. Simulate, metadata edit и межпроцессное управление playback остаются. |
| 16 | Bulk import, Android file access, playback queue | Ожидает |
| 17 | Полный контракт, regression/chaos/live suites | `BackendRequest/Response` используются реальным `backend_invoke` и генератором схем; покрыты auth/profiles/discovery/library/export/playback и локальные запросы. Строгий version gate, decimal IDs и schema drift проверены. Полная матрица A01–G10/chaos/live ещё не закрыта. |
| 18 | CI, сборки, упаковка, инструкции | Linux workspace checks проходят; API/CLI описаны в api.md. Добавлен check-android-rust.sh: arm64 native library действительно линкуется, проверены AAudio/JNI/16 KiB LOAD alignment; минимум API 26 согласован с CPAL. CI, полный APK с TDLib, Windows build, packaging, физическое аудио и общая инструкция ещё требуют работы. |

Пользователь подтвердил отсутствие отдельной Telegram-тестовой конфигурации: разрешены реализация и локальные проверки. Реальные account operations не запускать; их приёмку не объявлять завершённой по симулятору.

Проверки текущей серии изменений: `scripts/check-backend.sh` — успешно: format/check/Clippy, 98 passed, 0 failed, 1 ignored (opt-in real TDLib), CLI smoke и совпадение трёх JSON Schema с типами исполняемого IPC. Android plugin собирается с minSdk 26/targetSdk 36; все 12 instrumentation tests проходят на API 35, Lint без ошибок (два замечания о версиях тестовых зависимостей). `scripts/check-android-rust.sh` успешно собирает/линкует весь Rust app в arm64 .so, проверяет AAudio/JNI/16 KiB alignment. Во время device tests исправлены проверка штатного alias каталога Android и гонка stopSelf → новый player до onDestroy; отдельно проверены отказ подменённого secret record и устаревших управляющих команд. Это локальная проверка реализации, не завершение всей приёмки.

Сценарии смены плеера и background resume из уведомления прошли ещё пять совместных повторов (10 дополнительных выполнений). Повторные прогоны выявили гонку публикации foreground/paused уведомлений; исправлена единая последовательность обновления уведомления перед detach. После изменения tauri.conf также прошёл `cargo clippy --locked --offline -p kiyora --all-targets -- -D warnings`; frontend diff пуст.
