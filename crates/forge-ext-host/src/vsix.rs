//! Installing a `.vsix`: a zip whose `extension/` folder is the extension.

use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{bail, Context, Result};

use crate::{discover, load, Extension};

/// Refuse archives that would unpack to more than this (a zip bomb guard).
const MAX_UNPACKED_BYTES: u64 = 512 * 1024 * 1024;
const MAX_ENTRIES: usize = 50_000;

/// Unpack `vsix` into `extensions_dir/<publisher>.<name>-<version>` and return it.
/// Other installed versions of the same extension are removed. Nothing is left behind
/// if unpacking fails.
pub fn install_vsix(vsix: &Path, extensions_dir: &Path) -> Result<Extension> {
    let file = File::open(vsix).with_context(|| format!("opening {}", vsix.display()))?;
    let mut archive = zip::ZipArchive::new(file).context("not a valid .vsix (zip) file")?;
    if archive.len() > MAX_ENTRIES {
        bail!("the archive has {} entries (limit {MAX_ENTRIES})", archive.len());
    }
    std::fs::create_dir_all(extensions_dir).with_context(|| format!("creating {}", extensions_dir.display()))?;

    // Unique per install, so two installs running at once never share a folder.
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let staging = extensions_dir.join(format!(".installing-{}-{}", std::process::id(), COUNTER.fetch_add(1, Ordering::Relaxed)));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging)?;
    let result = unpack(&mut archive, &staging).and_then(|()| finish(&staging, extensions_dir));
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&staging);
    }
    result
}

fn unpack(archive: &mut zip::ZipArchive<File>, staging: &Path) -> Result<()> {
    let mut total = 0u64;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        // `enclosed_name` rejects absolute paths and `..` (zip-slip).
        let Some(name) = entry.enclosed_name() else { bail!("the archive contains an unsafe path: {}", entry.name()) };
        let Ok(inside) = name.strip_prefix("extension") else { continue };
        if inside.as_os_str().is_empty() {
            continue;
        }
        let target = staging.join(inside);
        if entry.is_dir() {
            std::fs::create_dir_all(&target)?;
            continue;
        }
        total = total.saturating_add(entry.size());
        if total > MAX_UNPACKED_BYTES {
            bail!("the extension unpacks to more than {} MiB", MAX_UNPACKED_BYTES >> 20);
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut out = File::create(&target).with_context(|| format!("writing {}", target.display()))?;
        // Trust the declared size no further than the limit.
        let mut limited = (&mut entry).take(MAX_UNPACKED_BYTES);
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let n = limited.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            out.write_all(&buffer[..n])?;
        }
    }
    Ok(())
}

fn finish(staging: &Path, extensions_dir: &Path) -> Result<Extension> {
    if !staging.join("package.json").is_file() {
        bail!("the archive has no extension/package.json");
    }
    let extension = load(staging)?;
    let id = extension.id();
    if id.is_empty() || id.contains(['/', '\\', ':']) || id.starts_with('.') {
        bail!("the extension id {id:?} is not a valid folder name");
    }
    let version = if extension.manifest.version.is_empty() { "0.0.0".to_owned() } else { extension.manifest.version.clone() };
    if version.contains(['/', '\\', ':']) {
        bail!("the extension version {version:?} is not a valid folder name");
    }

    // Replace every installed version of this extension.
    if let Ok((installed, _)) = discover(extensions_dir) {
        for old in installed.into_iter().filter(|e| e.id().eq_ignore_ascii_case(&id)) {
            let _ = std::fs::remove_dir_all(&old.root);
        }
    }
    let destination: PathBuf = extensions_dir.join(format!("{id}-{version}"));
    let _ = std::fs::remove_dir_all(&destination);
    std::fs::rename(staging, &destination).with_context(|| format!("moving into {}", destination.display()))?;
    load(&destination)
}

#[cfg(test)]
pub(crate) mod tests {
    use std::io::Write;

    use super::*;

    /// Write a `.vsix` with the given `(path, contents)` entries.
    pub(crate) fn make_vsix(path: &Path, entries: &[(&str, &str)]) {
        let mut writer = zip::ZipWriter::new(File::create(path).unwrap());
        let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        for (name, contents) in entries {
            writer.start_file(*name, options).unwrap();
            writer.write_all(contents.as_bytes()).unwrap();
        }
        writer.finish().unwrap();
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("forge-vsix-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    const MANIFEST: &str = r#"{ "name": "night", "publisher": "acme", "version": "1.0.0", "contributes": { "themes": [{ "label": "Night", "path": "./themes/night.json" }] } }"#;

    #[test]
    fn installs_the_extension_folder_and_ignores_the_rest_of_the_archive() {
        let dir = scratch("ok");
        let vsix = dir.join("night.vsix");
        make_vsix(&vsix, &[("[Content_Types].xml", "x"), ("extension.vsixmanifest", "x"), ("extension/package.json", MANIFEST), ("extension/themes/night.json", "{}")]);
        let extensions = dir.join("extensions");
        let installed = install_vsix(&vsix, &extensions).unwrap();
        assert_eq!(installed.id(), "acme.night");
        assert!(extensions.join("acme.night-1.0.0").join("themes").join("night.json").is_file());
        assert!(!extensions.join("acme.night-1.0.0").join("extension.vsixmanifest").exists());
        assert_eq!(std::fs::read_dir(&extensions).unwrap().count(), 1, "no staging folder is left behind");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn reinstalling_replaces_older_versions() {
        let dir = scratch("upgrade");
        let extensions = dir.join("extensions");
        let v1 = dir.join("v1.vsix");
        let v2 = dir.join("v2.vsix");
        make_vsix(&v1, &[("extension/package.json", MANIFEST)]);
        make_vsix(&v2, &[("extension/package.json", &MANIFEST.replace("1.0.0", "2.0.0"))]);
        install_vsix(&v1, &extensions).unwrap();
        install_vsix(&v2, &extensions).unwrap();
        let (found, _) = discover(&extensions).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].manifest.version, "2.0.0");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn rejects_unsafe_and_broken_archives_without_leaving_files() {
        let dir = scratch("bad");
        let extensions = dir.join("extensions");

        let slip = dir.join("slip.vsix");
        make_vsix(&slip, &[("extension/package.json", MANIFEST), ("extension/../../escape.txt", "x")]);
        assert!(install_vsix(&slip, &extensions).is_err());
        assert!(!dir.join("escape.txt").exists() && !extensions.join("escape.txt").exists());

        let no_manifest = dir.join("none.vsix");
        make_vsix(&no_manifest, &[("extension/readme.md", "x")]);
        assert!(install_vsix(&no_manifest, &extensions).unwrap_err().to_string().contains("package.json"));

        let bad_id = dir.join("id.vsix");
        make_vsix(&bad_id, &[("extension/package.json", r#"{ "name": "../evil", "publisher": "x" }"#)]);
        assert!(install_vsix(&bad_id, &extensions).is_err());

        let not_zip = dir.join("not.vsix");
        std::fs::write(&not_zip, "plain text").unwrap();
        assert!(install_vsix(&not_zip, &extensions).is_err());

        let leftovers = std::fs::read_dir(&extensions).map(|d| d.count()).unwrap_or(0);
        assert_eq!(leftovers, 0, "failed installs leave nothing behind");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
