# KIYORA Flutter client — migration in progress

The native client uses `kiyora-flutter-bridge` and the existing Rust backend.
Do not run or publish this directory as a completed replacement yet: the SDK,
generated bindings, platform runners and presentation layer are still being wired.

See [architecture and build progress](../docs/FLUTTER_MIGRATION.md),
[UI acceptance](../docs/FLUTTER_UI_ACCEPTANCE.md), and
[release gates](../docs/FLUTTER_RELEASE_READINESS.md).

Pinned SDK: Flutter 3.47.9 / Dart 3.13.7. Pinned FRB: 2.13.0 on both sides.
`flutter_rust_bridge.yaml` describes the limited API surface to generate.

Tests must use a temporary `InitOptions.dataDirectory`. Never point automated
checks at the default personal library or an existing Telegram account.
