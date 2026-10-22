#![allow(clippy::unwrap_used)]
use tauri::{
    ipc::{CallbackFn, InvokeBody},
    test::{INVOKE_KEY, get_ipc_response, mock_builder, mock_context, noop_assets},
    webview::InvokeRequest,
};

#[test]
fn webview_cannot_read_or_write_secrets_even_with_an_accidental_acl_grant() {
    for grant in [false, true] {
        let mut context = mock_context(noop_assets());
        let commands = [
            "getSecret",
            "setSecret",
            "get_secret",
            "set_secret",
            "storagePaths",
        ];
        if grant {
            for command in commands {
                context.runtime_authority_mut().__allow_command(
                    format!("plugin:kiyora-native|{command}"),
                    Default::default(),
                );
            }
        }
        let app = mock_builder()
            .plugin(tauri_plugin_kiyora_native::init())
            .build(context)
            .unwrap();
        let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .unwrap();
        for command in commands {
            let error = get_ipc_response(
                &webview,
                InvokeRequest {
                    cmd: format!("plugin:kiyora-native|{command}"),
                    callback: CallbackFn(0),
                    error: CallbackFn(1),
                    url: if cfg!(windows) {
                        "http://tauri.localhost"
                    } else {
                        "tauri://localhost"
                    }
                    .parse()
                    .unwrap(),
                    body: InvokeBody::Json(serde_json::json!({ "key": "private", "value": [42] })),
                    headers: Default::default(),
                    invoke_key: INVOKE_KEY.into(),
                },
            )
            .unwrap_err();
            if grant {
                assert_eq!(error, "native storage is accessible only from Rust");
            }
        }
    }
}
