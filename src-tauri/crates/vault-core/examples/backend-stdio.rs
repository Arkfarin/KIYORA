//! A persistent JSON-lines application boundary for opt-in integration runners.
//! stdin/stdout may contain credentials and private data: never enable wire logs.
use std::io::{BufRead, Write};
use tokio_util::sync::CancellationToken;
use vault_core::{ApiError, Backend, BackendRequest, BackendResponse, ErrorCode};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = std::env::var_os("KIYORA_TEST_ROOT").ok_or("KIYORA_TEST_ROOT required")?;
    let native = std::env::var_os("KIYORA_TDJSON").ok_or("KIYORA_TDJSON required")?;
    let backend = Backend::new(vault_security::ProfileStore::at(root)?, native);
    loop {
        let line = tokio::task::spawn_blocking(|| {
            let mut line = Vec::new();
            std::io::Read::take(std::io::stdin().lock(), 1024 * 1024 + 1)
                .read_until(b'\n', &mut line)?;
            Ok::<_, std::io::Error>(line)
        })
        .await??;
        if line.is_empty() {
            break;
        }
        if line.len() > 1024 * 1024 {
            break;
        }
        let request = serde_json::from_slice::<BackendRequest>(&line);
        let response = match request {
            Ok(request) => backend.invoke(request, &CancellationToken::new()).await,
            Err(_) => BackendResponse {
                version: 1,
                result: Err(ApiError::new(ErrorCode::InvalidInput)),
            },
        };
        serde_json::to_writer(std::io::stdout().lock(), &response)?;
        println!();
        std::io::stdout().flush()?;
    }
    backend.shutdown().await?;
    Ok(())
}
