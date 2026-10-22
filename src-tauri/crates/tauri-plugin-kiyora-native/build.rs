fn main() {
    // No frontend commands or default permission. Rust calls the mobile bridge
    // directly; no capability can grant access to secret storage.
    tauri_plugin::Builder::new(&[])
        .android_path("android")
        .build();
}
