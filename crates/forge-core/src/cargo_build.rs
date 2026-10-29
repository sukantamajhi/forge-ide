//! Building a Rust project with `cargo` so its program can be debugged.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::Value;

/// What a build produced: the program to debug, or why there is none, plus cargo's own messages.
#[derive(Debug)]
pub struct BuildOutcome {
    pub executable: Result<PathBuf, String>,
    /// Cargo's progress and compiler messages, as text.
    pub log: String,
    /// The package folder (where `Cargo.toml` is).
    pub package_dir: PathBuf,
}

/// The nearest folder above `file` that has a `Cargo.toml`.
pub fn package_dir(file: &Path) -> Option<PathBuf> {
    file.ancestors().skip(1).find(|dir| dir.join("Cargo.toml").is_file()).map(Path::to_path_buf)
}

/// The executable to debug among the artifacts of `cargo build --message-format=json*`: the binary
/// whose source is `file`, else a binary of the package that contains `file`, else the first binary.
pub fn pick_executable(cargo_output: &str, file: &Path) -> Option<PathBuf> {
    let mut candidates: Vec<(PathBuf, PathBuf, PathBuf)> = Vec::new(); // (executable, source, package dir)
    for line in cargo_output.lines() {
        let Ok(message) = serde_json::from_str::<Value>(line) else { continue };
        if message["reason"] != "compiler-artifact" {
            continue;
        }
        let (Some(exe), Some(source)) = (message["executable"].as_str(), message["target"]["src_path"].as_str()) else { continue };
        if !message["target"]["kind"].as_array().is_some_and(|k| k.iter().any(|v| v == "bin")) {
            continue;
        }
        let manifest = message["manifest_path"].as_str().map(PathBuf::from).and_then(|m| m.parent().map(Path::to_path_buf)).unwrap_or_default();
        candidates.push((PathBuf::from(exe), PathBuf::from(source), manifest));
    }
    let same = |a: &Path, b: &Path| a.to_string_lossy().replace('\\', "/").eq_ignore_ascii_case(&b.to_string_lossy().replace('\\', "/"));
    candidates
        .iter()
        .find(|(_, source, _)| same(source, file))
        .or_else(|| candidates.iter().find(|(_, _, package)| !package.as_os_str().is_empty() && file.starts_with(package)))
        .or_else(|| candidates.first())
        .map(|(exe, ..)| exe.clone())
}

/// Run `cargo build` in the package that contains `file` (blocking).
pub fn build(file: &Path) -> BuildOutcome {
    let Some(package_dir) = package_dir(file) else {
        return BuildOutcome { executable: Err("this file is not inside a Cargo project".to_owned()), log: String::new(), package_dir: PathBuf::new() };
    };
    let mut cmd = Command::new("cargo");
    cmd.args(["build", "--message-format=json-render-diagnostics"]).current_dir(&package_dir).stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let output = match cmd.output() {
        Ok(output) => output,
        Err(error) => return BuildOutcome { executable: Err(format!("could not run cargo: {error}")), log: String::new(), package_dir },
    };
    let log = String::from_utf8_lossy(&output.stderr).into_owned();
    let executable = if output.status.success() {
        pick_executable(&String::from_utf8_lossy(&output.stdout), file).ok_or_else(|| "the build produced no program to run (is this a library?)".to_owned())
    } else {
        Err("the build failed".to_owned())
    };
    BuildOutcome { executable, log, package_dir }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn artifact(exe: &str, src: &str, manifest: &str, kind: &str) -> String {
        serde_json::json!({
            "reason": "compiler-artifact",
            "manifest_path": manifest,
            "target": { "kind": [kind], "src_path": src },
            "executable": if exe.is_empty() { Value::Null } else { Value::from(exe) },
        })
        .to_string()
    }

    #[test]
    fn the_binary_for_the_active_file_wins() {
        let output = [
            artifact("", "/w/lib/src/lib.rs", "/w/lib/Cargo.toml", "lib"),
            artifact("/t/debug/one", "/w/app/src/bin/one.rs", "/w/app/Cargo.toml", "bin"),
            artifact("/t/debug/app", "/w/app/src/main.rs", "/w/app/Cargo.toml", "bin"),
            r#"{"reason":"build-finished","success":true}"#.to_owned(),
        ]
        .join("\n");
        assert_eq!(pick_executable(&output, Path::new("/w/app/src/main.rs")), Some(PathBuf::from("/t/debug/app")));
        assert_eq!(pick_executable(&output, Path::new("/w/app/src/bin/one.rs")), Some(PathBuf::from("/t/debug/one")));
        // A helper module in the package picks one of the package's binaries.
        assert_eq!(pick_executable(&output, Path::new("/w/app/src/util.rs")), Some(PathBuf::from("/t/debug/one")));
        // A library alone has nothing to run.
        assert_eq!(pick_executable(&artifact("", "/w/lib/src/lib.rs", "/w/lib/Cargo.toml", "lib"), Path::new("/w/lib/src/lib.rs")), None);
    }

    #[test]
    fn source_paths_match_regardless_of_slashes_and_case() {
        let output = artifact("C:\\t\\app.exe", "C:\\W\\app\\src\\main.rs", "C:\\W\\app\\Cargo.toml", "bin");
        assert_eq!(pick_executable(&output, Path::new("c:/w/app/src/main.rs")), Some(PathBuf::from("C:\\t\\app.exe")));
    }

    #[test]
    fn the_package_is_the_nearest_folder_with_a_cargo_toml() {
        let root = std::env::temp_dir().join(format!("forge-pkg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("crates/a/src")).unwrap();
        std::fs::write(root.join("Cargo.toml"), "").unwrap();
        std::fs::write(root.join("crates/a/Cargo.toml"), "").unwrap();
        assert_eq!(package_dir(&root.join("crates/a/src/main.rs")), Some(root.join("crates/a")));
        assert_eq!(package_dir(&root.join("README.md")), Some(root.clone()));
        let _ = std::fs::remove_dir_all(&root);
    }
}
