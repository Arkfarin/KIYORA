#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
cargo fmt --all -- --check
cargo check --locked --workspace
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
cargo run --locked -q -p vault-core --bin kiyora-cli -- --help | grep -q 'profiles list \[--json\]'
schema_tmp="$(mktemp -d)"
trap 'rm -rf "$schema_tmp"' EXIT
cargo run --locked -q -p vault-core --example generate-api-schema -- "$schema_tmp"
cmp spec/schema/backend-request-v1.schema.json "$schema_tmp/backend-request-v1.schema.json"
cmp spec/schema/backend-response-v1.schema.json "$schema_tmp/backend-response-v1.schema.json"
cmp spec/schema/backend-event-v1.schema.json "$schema_tmp/backend-event-v1.schema.json"
if [[ -n "${KIYORA_TDJSON:-}" ]]; then
  cargo test --locked -p vault-telegram --test real_tdlib -- --ignored --exact real_native_start_version_close
fi
