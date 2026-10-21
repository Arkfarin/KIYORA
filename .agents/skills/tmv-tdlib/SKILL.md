---
name: tmv-tdlib
description: Implement, debug, or audit the Telegram Music Vault TDLib tdjson adapter, account authorization, native FFI, Telegram channel discovery, message history, file uploads/downloads and real integration tests. Use for Rust code that touches Telegram user sessions or TDLib APIs; do not use for pure domain reducer logic.
---

# TMV: TDLib implementation and verification

Work from `AGENTS.md` and the project's `telegram_music_architecture_ru(1).md` (or renamed equivalent), especially §§2, 4, 7, 8, 11–13, 17, 20, 24. Treat application-service pseudocode in the RFC as conceptual, not TDLib methods.

## Workflow

1. Check the repository's TDLib version, linking/loading strategy, target triples and actual bindings. Verify native API names and request/response fields against the **matching TDLib version's official API documentation**; do not invent a method because it sounds plausible.
2. Encapsulate all C ABI/`tdjson` unsafe operations in a small audited module. Validate CString lifetime, client lifetime, ownership of native pointers, UTF-8/JSON processing, threading, shutdown and cancellation.
3. Implement request correlation using TDLib's `@extra` or the actual supported mechanism, plus a distinct update stream. Handle timeouts, send completions, temporary and final IDs and graceful close.
4. Model `updateAuthorizationState` as a state machine. Support code and 2FA; add QR only via documented supported TDLib flow. No phone/code/password in logs or exports.
5. Confirm Telegram identity (`getMe`), permissions/ownership and vault marker; don't trust channel titles or search incompleteness as proof a vault does not exist.
6. For chat history, handle reverse-ordered pages shorter than the requested limit, numeric/noncontiguous server IDs, duplicate/out-of-order updates, rate limits and cancellation. Expose an abstract transport trait for deterministic tests.
7. For media, separate transport references, remote metadata, chunked downloads, progress updates and local caching; do not assume TDLib file IDs are stable global identities.
8. Keep a dedicated, opt-in test-account integration suite. No automatic destructive operations on a user's real channel.

## Deliverables and verification

- Typed Telegram adapter API, unit tests with fake transport, clear native-library install/build notes and integration test instructions.
- Explicit tested outcomes for each target (Linux, Windows, Android), distinguishing compile-only checks from live TDLib execution.
- Never mark `login`, `upload`, or `download` working unless the actual implementation exists and the corresponding behavior was exercised or clearly recorded as unverified.
