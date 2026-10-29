//! Installing debug adapters into Forge's own folder: js-debug (JavaScript and Node) and CodeLLDB
//! (Rust and C/C++).
//!
//! Only runs when the user asks. It reads the latest release of each project from GitHub, refuses a
//! download that does not come from that project's release page, checks the published SHA-256 before
//! unpacking anything, and replaces the adapter's folder as a whole so a failed install leaves the
//! old one in place.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::Value;
use sha2::{Digest, Sha256};

/// Where an adapter comes from and what its archive looks like.
struct Source {
    label: &'static str,
    release_api: &'static str,
    download_prefix: &'static str,
    /// Matches the archive's file name in the release.
    asset: fn(&str) -> bool,
    /// The folder under `<servers>` it unpacks into.
    folder: &'static str,
    zip: bool,
    /// A file that must exist after unpacking, relative to the folder.
    marker: &'static [&'static str],
}

fn codelldb_asset(name: &str) -> bool {
    let wanted = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", _) => "codelldb-win32-x64.vsix",
        ("macos", "aarch64") => "codelldb-darwin-arm64.vsix",
        ("macos", _) => "codelldb-darwin-x64.vsix",
        (_, "aarch64") => "codelldb-linux-arm64.vsix",
        _ => "codelldb-linux-x64.vsix",
    };
    name == wanted
}

const JS_DEBUG: Source = Source {
    label: "the JavaScript debugger",
    release_api: "https://api.github.com/repos/microsoft/vscode-js-debug/releases/latest",
    download_prefix: "https://github.com/microsoft/vscode-js-debug/releases/download/",
    asset: |n| n.starts_with("js-debug-dap-v") && n.ends_with(".tar.gz"),
    folder: "js-debug",
    zip: false,
    marker: &["js-debug", "src", "dapDebugServer.js"],
};

const CODELLDB: Source = Source {
    label: "the Rust debugger (CodeLLDB)",
    release_api: "https://api.github.com/repos/vadimcn/codelldb/releases/latest",
    download_prefix: "https://github.com/vadimcn/codelldb/releases/download/",
    asset: codelldb_asset,
    folder: "codelldb",
    zip: true,
    marker: &["extension", "adapter", if cfg!(windows) { "codelldb.exe" } else { "codelldb" }],
};

fn quiet(cmd: &mut Command) -> &mut Command {
    cmd.stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    cmd
}

fn curl(args: &[&str]) -> Result<Vec<u8>, String> {
    let output = quiet(Command::new("curl").args(["--silent", "--show-error", "--location", "--fail", "--max-time", "900"]).args(args)).output().map_err(|e| format!("could not run curl: {e}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).lines().next().unwrap_or("the download failed").to_owned());
    }
    Ok(output.stdout)
}

/// The tag, download URL and SHA-256 of the archive `matches` picks from a GitHub release listing.
fn pick_asset(release: &Value, prefix: &str, matches: fn(&str) -> bool) -> Result<(String, String, String), String> {
    let tag = release["tag_name"].as_str().ok_or("the release has no version")?.to_owned();
    let asset = release["assets"].as_array().into_iter().flatten().find(|a| a["name"].as_str().is_some_and(matches)).ok_or("the release has no download for this computer")?;
    let url = asset["browser_download_url"].as_str().ok_or("the archive has no download link")?;
    if !url.starts_with(prefix) || url.contains(['@', ' ', '\\']) {
        return Err(format!("refusing to download from {url}"));
    }
    let digest = asset["digest"].as_str().and_then(|d| d.strip_prefix("sha256:")).ok_or("the release publishes no checksum, so the download cannot be verified")?;
    Ok((tag, url.to_owned(), digest.to_ascii_lowercase()))
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// Download, verify and unpack js-debug into `<servers_dir>/js-debug`. Returns a message for the status bar.
pub fn install_js_debug(servers_dir: &Path) -> Result<String, String> {
    install(servers_dir, &JS_DEBUG)
}

/// Download, verify and unpack CodeLLDB into `<servers_dir>/codelldb`.
pub fn install_codelldb(servers_dir: &Path) -> Result<String, String> {
    install(servers_dir, &CODELLDB)
}

fn install(servers_dir: &Path, source: &Source) -> Result<String, String> {
    let listing = curl(&[source.release_api])?;
    let release: Value = serde_json::from_slice(&listing).map_err(|_| "unexpected reply from GitHub".to_owned())?;
    let (tag, url, expected) = pick_asset(&release, source.download_prefix, source.asset)?;

    let work = std::env::temp_dir().join(format!("forge-adapter-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos())));
    std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    let result = unpack(servers_dir, &work, &url, &expected, source);
    let _ = std::fs::remove_dir_all(&work);
    result.map(|()| format!("Installed {} ({tag})", source.label))
}

fn unpack(servers_dir: &Path, work: &Path, url: &str, expected: &str, source: &Source) -> Result<(), String> {
    let archive = curl(&[url])?;
    let actual = sha256_hex(&archive);
    if actual != expected {
        return Err(format!("the download does not match its published checksum (got {actual}), so it was discarded"));
    }
    let file = work.join("archive");
    std::fs::write(&file, &archive).map_err(|e| e.to_string())?;

    let staging: PathBuf = servers_dir.join(format!("{}.new", source.folder));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging).map_err(|e| e.to_string())?;
    let unpacked = if source.zip { unzip(&file, &staging) } else { untar(&file, &staging) };
    let marker = source.marker.iter().fold(staging.clone(), |p, part| p.join(part));
    if let Err(error) = unpacked.and_then(|()| if marker.is_file() { Ok(()) } else { Err("the archive does not contain the debug adapter".to_owned()) }) {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(error);
    }
    let target = servers_dir.join(source.folder);
    let _ = std::fs::remove_dir_all(&target);
    std::fs::rename(&staging, &target).map_err(|e| format!("could not put the debugger in place: {e}"))
}

fn untar(file: &Path, into: &Path) -> Result<(), String> {
    let output = quiet(Command::new("tar").arg("-xzf").arg(file).arg("-C").arg(into)).output().map_err(|e| format!("could not run tar: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!("could not unpack the archive: {}", String::from_utf8_lossy(&output.stderr).lines().next().unwrap_or("tar failed")))
    }
}

/// Unpack a zip (a `.vsix`), refusing any entry that would land outside `into`.
fn unzip(file: &Path, into: &Path) -> Result<(), String> {
    let mut archive = zip::ZipArchive::new(std::fs::File::open(file).map_err(|e| e.to_string())?).map_err(|e| format!("could not read the archive: {e}"))?;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|e| e.to_string())?;
        let Some(relative) = entry.enclosed_name() else { return Err(format!("the archive has an unsafe path: {}", entry.name())) };
        let out = into.join(relative);
        if entry.is_dir() {
            std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
            continue;
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let mut data = Vec::with_capacity(entry.size() as usize);
        entry.read_to_end(&mut data).map_err(|e| e.to_string())?;
        std::fs::write(&out, data).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        if let Some(mode) = entry.unix_mode() {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&out, std::fs::Permissions::from_mode(mode));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Write;

    fn release(name: &str, url: &str, digest: Option<&str>) -> Value {
        let mut asset = json!({ "name": name, "browser_download_url": url });
        if let Some(d) = digest {
            asset["digest"] = json!(d);
        }
        json!({ "tag_name": "v1.2.3", "assets": [{ "name": "other.vsix", "browser_download_url": "https://example.com/x" }, asset] })
    }

    #[test]
    fn the_archive_asset_is_found_with_its_checksum() {
        let good = format!("{}v1.2.3/js-debug-dap-v1.2.3.tar.gz", JS_DEBUG.download_prefix);
        let (tag, url, digest) = pick_asset(&release("js-debug-dap-v1.2.3.tar.gz", &good, Some("sha256:ABCDEF")), JS_DEBUG.download_prefix, JS_DEBUG.asset).unwrap();
        assert_eq!((tag.as_str(), url.as_str(), digest.as_str()), ("v1.2.3", good.as_str(), "abcdef"));
    }

    #[test]
    fn downloads_from_anywhere_else_or_without_a_checksum_are_refused() {
        let prefix = JS_DEBUG.download_prefix;
        let name = "js-debug-dap-v1.tar.gz";
        assert!(pick_asset(&release(name, "https://evil.example/js-debug-dap-v1.tar.gz", Some("sha256:aa")), prefix, JS_DEBUG.asset).unwrap_err().contains("refusing"));
        // Another project's release page is not this project's.
        let other = format!("{}v1/{name}", CODELLDB.download_prefix);
        assert!(pick_asset(&release(name, &other, Some("sha256:aa")), prefix, JS_DEBUG.asset).is_err());
        let sneaky = format!("{prefix}@evil.example/a.tar.gz");
        assert!(pick_asset(&release(name, &sneaky, Some("sha256:aa")), prefix, JS_DEBUG.asset).is_err());
        let good = format!("{prefix}v1/{name}");
        assert!(pick_asset(&release(name, &good, None), prefix, JS_DEBUG.asset).unwrap_err().contains("checksum"));
        assert!(pick_asset(&json!({ "tag_name": "v1", "assets": [] }), prefix, JS_DEBUG.asset).is_err());
    }

    #[test]
    fn only_this_computers_codelldb_archive_is_chosen() {
        let names = ["codelldb-darwin-arm64.vsix", "codelldb-darwin-x64.vsix", "codelldb-linux-arm64.vsix", "codelldb-linux-x64.vsix", "codelldb-win32-x64.vsix", "codelldb-bootstrap.vsix"];
        assert_eq!(names.iter().filter(|n| codelldb_asset(n)).count(), 1);
    }

    #[test]
    fn sha256_matches_a_known_value() {
        assert_eq!(sha256_hex(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }

    #[test]
    fn unzip_extracts_files_and_refuses_paths_that_escape() {
        let dir = std::env::temp_dir().join(format!("forge-unzip-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let build = |name: &str| {
            let path = dir.join("test.zip");
            let mut writer = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
            writer.start_file(name, zip::write::SimpleFileOptions::default()).unwrap();
            writer.write_all(b"data").unwrap();
            writer.finish().unwrap();
            path
        };
        let out = dir.join("out");
        std::fs::create_dir_all(&out).unwrap();
        unzip(&build("extension/adapter/tool.txt"), &out).unwrap();
        assert_eq!(std::fs::read_to_string(out.join("extension/adapter/tool.txt")).unwrap(), "data");
        assert!(unzip(&build("../escape.txt"), &out).is_err());
        assert!(!dir.join("escape.txt").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Downloads for real. Run with `cargo test -p forge-core --lib real_install -- --ignored`.
    #[test]
    #[ignore]
    fn real_install_into_a_scratch_folder() {
        let dir = std::env::temp_dir().join(format!("forge-adapter-install-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let message = install_js_debug(&dir).unwrap();
        assert!(message.starts_with("Installed the JavaScript debugger"), "{message}");
        assert!(dir.join("js-debug").join("js-debug").join("src").join("dapDebugServer.js").is_file());
        assert!(!dir.join("js-debug.new").exists());
        let message = install_codelldb(&dir).unwrap();
        assert!(message.starts_with("Installed the Rust debugger"), "{message}");
        assert!(codelldb_present(&dir));
        let _ = std::fs::remove_dir_all(&dir);
    }

    pub(super) fn codelldb_present(dir: &Path) -> bool {
        CODELLDB.marker.iter().fold(dir.join("codelldb"), |p, part| p.join(part)).is_file()
    }
}
