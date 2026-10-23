use std::{future::Future, path::PathBuf, pin::Pin};
use tauri::{Runtime, plugin::PluginHandle};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use vault_core::{ApiError, ErrorCode, ImportResolver};
#[derive(serde::Deserialize)]
struct Resolved {
    path: PathBuf,
}
pub struct AndroidDocuments<R: Runtime>(PluginHandle<R>);
impl<R: Runtime> Clone for AndroidDocuments<R> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl<R: Runtime> AndroidDocuments<R> {
    pub(crate) fn new(handle: PluginHandle<R>) -> Self {
        Self(handle)
    }
}
impl<R: Runtime> ImportResolver for AndroidDocuments<R> {
    fn resolve(
        &self,
        uri: String,
        op_id: Uuid,
        cancel: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Result<PathBuf, ApiError>> + Send + '_>> {
        Box::pin(async move {
            if cancel.is_cancelled() {
                return Err(ApiError::new(ErrorCode::Cancelled));
            }
            let request = self.0.run_mobile_plugin_async::<Resolved>(
                "stageDocument",
                serde_json::json!({"uri":uri,"op_id":op_id}),
            );
            tokio::pin!(request);
            tokio::select! {
                result=&mut request=>result.map(|r|r.path).map_err(|_|ApiError::new(ErrorCode::Storage)),
                _=cancel.cancelled()=>{
                    let _:Result<(),_>=self.0.run_mobile_plugin_async("cancelDocument",serde_json::json!({"op_id":op_id})).await;
                    let _=request.await;
                    Err(ApiError::new(ErrorCode::Cancelled))
                }
            }
        })
    }
    fn release(
        &self,
        op_id: Uuid,
    ) -> Pin<Box<dyn Future<Output = Result<(), ApiError>> + Send + '_>> {
        Box::pin(async move {
            self.0
                .run_mobile_plugin_async::<()>(
                    "releaseDocument",
                    serde_json::json!({"op_id":op_id}),
                )
                .await
                .map_err(|_| ApiError::new(ErrorCode::Storage))
        })
    }
}
