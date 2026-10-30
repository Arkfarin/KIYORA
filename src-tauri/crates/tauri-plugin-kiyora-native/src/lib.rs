//! Native platform services for Rust only. Never exposes secrets to the WebView.
use tauri::{Runtime, plugin::TauriPlugin};

#[cfg(target_os = "android")]
mod android;
#[cfg(target_os = "android")]
pub use android::AndroidSecrets;
#[cfg(target_os = "android")]
mod media;
#[cfg(target_os = "android")]
pub use media::AndroidMedia;
#[cfg(target_os = "android")]
mod documents;
#[cfg(target_os = "android")]
pub use documents::AndroidDocuments;

#[cfg(any(target_os = "linux", target_os = "windows"))]
mod desktop_media;
#[cfg(any(target_os = "linux", target_os = "windows"))]
pub use desktop_media::DesktopMedia;

pub fn init<R: Runtime>() -> TauriPlugin<R> {
    tauri::plugin::Builder::new("kiyora-native")
        // Defense in depth: deny even if a future capability grants the native
        // command. Returning true prevents Tauri's mobile command fallback.
        .invoke_handler(|invoke| {
            invoke
                .resolver
                .reject("native storage is accessible only from Rust");
            true
        })
        .setup(|_app, _api| {
            #[cfg(target_os = "android")]
            {
                use tauri::Manager;
                let handle = _api.register_android_plugin(
                    "io.github.arkfarin.kiyora.nativebridge",
                    "KiyoraNativePlugin",
                )?;
                _app.manage(AndroidSecrets::new(handle.clone()));
                _app.manage(AndroidDocuments::new(handle.clone()));
                _app.manage(AndroidMedia::new(handle));
            }
            Ok(())
        })
        .build()
}
