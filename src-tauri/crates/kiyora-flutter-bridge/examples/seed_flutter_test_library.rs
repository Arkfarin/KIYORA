//! Test-only fixture preparation. Never linked into the Flutter native library.
#[path = "../tests/support/mod.rs"]
mod support;

#[tokio::main]
async fn main() -> support::TestResult {
    let mut args = std::env::args_os().skip(1);
    let root = std::path::PathBuf::from(args.next().ok_or("provide an empty test directory")?);
    let count: u32 = args.next().map(|v| v.to_string_lossy().parse()).transpose()?.unwrap_or(12);
    if root.exists() && std::fs::read_dir(&root)?.next().is_some() {
        return Err("fixture directory must be empty; existing data is never replaced".into());
    }
    if count > 10_001 { return Err("fixture limit is 10001 tracks".into()); }
    std::fs::create_dir_all(&root)?;
    let (init, tracks) = support::fixture(&root, count).await?;
    kiyora_flutter_bridge::api::lifecycle::shutdown(init.client_id).await?;
    println!("{}", serde_json::json!({"tracks": tracks}));
    Ok(())
}
