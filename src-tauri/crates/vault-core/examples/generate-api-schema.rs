use schemars::JsonSchema;
use serde::Serialize;
use std::path::PathBuf;
use vault_core::{PlaybackAction, VaultRequest};

#[derive(JsonSchema, Serialize)]
#[serde(tag = "scope", content = "request", rename_all = "snake_case")]
#[allow(dead_code)]
enum BackendRequest {
    Vault(VaultRequest),
    Playback(PlaybackAction),
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let destination = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("spec/schema/backend-request-v1.schema.json"));
    let schema = schemars::schema_for!(BackendRequest);
    let mut bytes = serde_json::to_vec_pretty(&schema)?;
    bytes.push(b'\n');
    std::fs::write(destination, bytes)?;
    Ok(())
}
