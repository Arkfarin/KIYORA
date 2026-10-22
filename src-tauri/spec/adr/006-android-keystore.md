# ADR 006: Android Keystore and the private native bridge

Status: accepted.

## Decision

`Backend::with_secrets` receives a thread-safe `SecretStore`; the desktop default
retains OS keyring storage. The Tauri Android shell injects `AndroidSecrets` from
the local `tauri-plugin-kiyora-native` crate. Secret operations run on blocking
Rust workers. The session lock remains held while keys/configuration are persisted,
so a profile switch cannot interleave with native session startup.

The Android plugin uses AndroidKeyStore AES-256-GCM keys, one alias per profile
and secret purpose. Only canonical non-nil profile UUIDs with `tdlib-key` or
`api-hash` suffixes and 32-byte values are accepted. Each ciphertext record has a
version byte, fresh provider-generated 96-bit nonce, ciphertext and 128-bit tag.
AAD binds the record to its version, profile UUID and purpose. Keystore keys are
not exported. Hardware backing depends on the Android device; it is not claimed
by the application or by emulator tests.

Ciphertext lives in the app's `noBackupFilesDir`, separately from the per-profile
TDLib/SQLite/cache tree, which also lives under `noBackupFilesDir`. Automatic
Android backup therefore cannot transplant a session without its Keystore keys.
Explicit portable backups continue to contain only the approved library/media
data. No plaintext fallback or implicit key reset exists.

Writes use Android `AtomicFile`, then read back and authenticate the result before
acknowledging success. An existing record must decrypt before replacement. Missing
Keystore keys, invalidated keys, corrupt ciphertext, unsupported versions, oversized
records and filesystem failures return a structured secure-storage error. Rust
also checks read-back persistence before allowing TDLib to create its database.

## Bridge boundary

There are no frontend permissions/default grants for this plugin. Its Rust invoke
handler unconditionally rejects WebView requests and reports them handled, which
prevents Tauri's native-command fallback even after an accidental ACL grant.
Only Rust `PluginHandle::run_mobile_plugin_async` calls reach native secret methods.
The IPC regression uses Tauri's mock runtime to exercise both ungranted and
explicitly granted WebView requests.

Native operations are serialized on one executor with a 16-request queue. Rust
waits at most 15 seconds. Timeout is an error; an already queued atomic write may
still finish. A retry authenticates/read-backs the durable state. No command logs
arguments, returned secrets, or exception text. Owned Rust buffers and native
byte arrays are wiped after use; temporary JSON/JNI/JVM copies cannot be guaranteed
to disappear immediately and are not described as zero-copy secure memory.

## Evidence and remaining limits

On 2026-10-22 the Kotlin library, instrumentation APK and Android Lint passed;
the Rust plugin passed `cargo check --target aarch64-linux-android`. Five tests
passed against real AndroidKeyStore in an Android 15/API 35 x86_64 AOSP emulator:
round-trip/reopen/random nonces, swapped/corrupted ciphertext without overwrite,
missing-key refusal, input/size bounds and AtomicFile interrupted-write recovery.

This does not prove hardware-backed storage, physical-device behavior, complete
Tauri application startup, Android TDLib packaging/login, or foreground playback.
Those remain separate acceptance gates. The Android CLI without the Tauri native
provider continues to fail closed rather than use an insecure secret store.
