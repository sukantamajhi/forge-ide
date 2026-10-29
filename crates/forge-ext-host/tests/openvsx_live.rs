//! Talks to the real open-vsx.org. Skipped when the network (or curl) is unavailable.

use forge_ext_host::{discover, openvsx, uninstall, vsix::install_vsix};

#[test]
fn a_real_theme_extension_downloads_verifies_and_installs() {
    let listing = match openvsx::lookup("dracula-theme.theme-dracula") {
        Ok(listing) => listing,
        Err(error) => {
            eprintln!("skipped: Open VSX is not reachable ({error})");
            return;
        }
    };
    assert_eq!(listing.id().to_lowercase(), "dracula-theme.theme-dracula");
    assert!(listing.sha256_url.is_some(), "the registry publishes a checksum");

    let dir = std::env::temp_dir().join(format!("forge-live-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let vsix = dir.join("theme.vsix");
    openvsx::download(&listing, &vsix).expect("download and checksum");

    let extensions = dir.join("extensions");
    let installed = install_vsix(&vsix, &extensions).expect("install");
    assert!(!installed.manifest.contributes.themes.is_empty(), "a theme extension contributes themes");
    let theme = &installed.manifest.contributes.themes[0];
    let path = installed.resolve(&theme.path).expect("a confined theme path");
    assert!(path.is_file(), "{} exists", path.display());
    eprintln!("installed {} with {} theme(s): {}", installed.id(), installed.manifest.contributes.themes.len(), installed.supported_summary());

    assert_eq!(discover(&extensions).unwrap().0.len(), 1);
    assert_eq!(uninstall(&extensions, &installed.id()).unwrap(), 1);
    assert_eq!(discover(&extensions).unwrap().0.len(), 0);
    std::fs::remove_dir_all(&dir).unwrap();
}
