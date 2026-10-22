#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
cargo fmt --all -- --check
cargo check --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
schema_tmp="$(mktemp)"
trap 'rm -f "$schema_tmp"' EXIT
cargo run -q -p vault-core --example generate-api-schema -- "$schema_tmp"
cmp spec/schema/backend-request-v1.schema.json "$schema_tmp"
if [[ -n "${KIYORA_TDJSON:-}" ]]; then
  cargo test -p vault-telegram --test real_tdlib -- --ignored --exact real_native_start_version_close
fi
