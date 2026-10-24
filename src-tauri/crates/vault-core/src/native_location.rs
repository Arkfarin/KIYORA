//! Installed native dependencies are resolved from the bundle or executable,
//! never from the shell's working directory or an implicit DLL search path.
use std::path::{Path, PathBuf};

fn filename() -> &'static str {
    if cfg!(target_os = "windows") {
        "tdjson.dll"
    } else if cfg!(target_os = "macos") {
        "libtdjson.dylib"
    } else {
        "libtdjson.so"
    }
}

pub(crate) fn resolve(resources: Option<&Path>) -> Result<PathBuf, crate::ApiError> {
    let explicit = std::env::var_os("KIYORA_TDJSON").map(PathBuf::from);
    let executable = std::env::current_exe()
        .map_err(|_| crate::ApiError::new(crate::ErrorCode::NativeUnavailable))?;
    Ok(locate(explicit, resources, &executable))
}

fn locate(explicit: Option<PathBuf>, resources: Option<&Path>, executable: &Path) -> PathBuf {
    if let Some(path) = explicit {
        return path;
    }
    let parent = executable.parent().unwrap_or(executable);
    let adjacent = parent.join("native").join(filename());
    let candidates = resources
        .map(|root| root.join("native").join(filename()))
        .into_iter()
        .chain([
            adjacent.clone(),
            parent.join(filename()),
            parent.join("../lib/kiyora/native").join(filename()),
        ]);
    candidates
        .into_iter()
        .find(|path| path.is_file())
        .unwrap_or(adjacent)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    #[test]
    fn bundle_and_cli_paths_are_resolved_without_a_working_directory_search() {
        let root = tempfile::tempdir().unwrap();
        let executable = root.path().join("bin/kiyora");
        let resources = root.path().join("resources");
        let bundled = resources.join("native").join(filename());
        let adjacent = root.path().join("bin/native").join(filename());
        let installed = root.path().join("lib/kiyora/native").join(filename());
        for path in [&bundled, &adjacent, &installed] {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, []).unwrap();
        }
        assert_eq!(locate(None, Some(&resources), &executable), bundled);
        assert_eq!(locate(None, None, &executable), adjacent);
        std::fs::remove_file(&adjacent).unwrap();
        assert_eq!(
            locate(None, None, &executable).canonicalize().unwrap(),
            installed
        );
        std::fs::remove_file(&installed).unwrap();
        assert_eq!(locate(None, None, &executable), adjacent);
        assert!(locate(None, None, &executable).is_absolute());
        let override_path = root.path().join("developer.so");
        assert_eq!(
            locate(Some(override_path.clone()), Some(&resources), &executable),
            override_path
        );
    }
}
