use schemars::JsonSchema;
use serde::Serialize;
use std::path::{Path, PathBuf};
use vault_core::{ApiError, BackendEvent, PlaybackAction, VaultRequest, VaultResponse};

#[derive(JsonSchema, Serialize)]
#[serde(tag = "scope", content = "request", rename_all = "snake_case")]
#[allow(dead_code)]
enum BackendRequest {
    Vault(VaultRequest),
    Playback(PlaybackAction),
}

#[derive(JsonSchema, Serialize)]
#[serde(tag = "scope", content = "response", rename_all = "snake_case")]
#[allow(dead_code)]
enum BackendResponse {
    Vault(VaultResponse),
    Error(ApiError),
}

fn write_schema<T: JsonSchema>(
    directory: &Path,
    filename: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let schema = schemars::schema_for!(T);
    let mut bytes = serde_json::to_vec_pretty(&schema)?;
    bytes.push(b'\n');
    std::fs::write(directory.join(filename), bytes)?;
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let directory = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("spec/schema"));
    std::fs::create_dir_all(&directory)?;
    write_schema::<BackendRequest>(&directory, "backend-request-v1.schema.json")?;
    write_schema::<BackendResponse>(&directory, "backend-response-v1.schema.json")?;
    write_schema::<BackendEvent>(&directory, "backend-event-v1.schema.json")?;
    Ok(())
}
