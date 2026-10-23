//! A persistent process for native clients, scripting and playback control.
//! Requests can contain credentials. Never log the raw stdin/stdout stream.
use crate::{ApiError, Backend, BackendRequest, BackendResponse, ErrorCode};
use std::io::{BufRead, Write};
use tokio_util::sync::CancellationToken;

pub async fn serve_stdio(backend: &Backend) -> Result<(), ApiError> {
    loop {
        let line = tokio::task::spawn_blocking(|| {
            let mut line = Vec::new();
            std::io::Read::take(std::io::stdin().lock(), 1024 * 1024 + 1)
                .read_until(b'\n', &mut line)?;
            Ok::<_, std::io::Error>(line)
        })
        .await
        .map_err(|_| ApiError::new(ErrorCode::Internal))??;
        if line.is_empty() {
            break;
        }
        if line.len() > 1024 * 1024 {
            return Err(ApiError::new(ErrorCode::InvalidInput));
        }
        let response = match serde_json::from_slice::<BackendRequest>(&line) {
            Ok(request) => backend.invoke(request, &CancellationToken::new()).await,
            Err(_) => BackendResponse {
                version: 1,
                result: Err(ApiError::new(ErrorCode::InvalidInput)),
            },
        };
        let encoded =
            serde_json::to_vec(&response).map_err(|_| ApiError::new(ErrorCode::Internal))?;
        tokio::task::spawn_blocking(move || -> std::io::Result<()> {
            let mut out = std::io::stdout().lock();
            out.write_all(&encoded)?;
            out.write_all(b"\n")?;
            out.flush()
        })
        .await
        .map_err(|_| ApiError::new(ErrorCode::Internal))??;
    }
    Ok(())
}
