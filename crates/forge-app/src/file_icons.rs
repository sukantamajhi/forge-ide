//! Which small coloured badge stands for a file in the explorer, tabs and Git list.
//! Names win over extensions (`Dockerfile`, `.env`, `Cargo.lock`).

/// The badge text and its colour (0xRRGGBB) for a file called `name`.
pub fn badge(name: &str) -> (&'static str, u32) {
    let lower = name.to_ascii_lowercase();
    match lower.as_str() {
        "dockerfile" | "containerfile" => return ("DK", 0x4a9fe0),
        "makefile" | "gnumakefile" => return ("MK", 0x9aa1ad),
        ".gitignore" | ".gitattributes" | ".gitmodules" | ".dockerignore" => return ("GI", 0xe8734a),
        ".editorconfig" => return ("EC", 0x9aa1ad),
        "license" | "licence" | "copying" => return ("LI", 0xd6a85a),
        "readme" | "readme.md" => return ("MD", 0x6bb5e0),
        _ => {}
    }
    if lower == ".env" || lower.starts_with(".env.") || lower.ends_with(".env") {
        return ("EN", 0xe8c547);
    }
    let ext = lower.rsplit_once('.').map_or("", |(_, e)| e);
    match ext {
        "rs" => ("RS", 0xe6955a),
        "py" | "pyi" | "pyw" => ("PY", 0x6aa8e0),
        "js" | "mjs" | "cjs" => ("JS", 0xe8d44d),
        "jsx" => ("JX", 0x61dafb),
        "ts" | "mts" | "cts" => ("TS", 0x5b9bd5),
        "tsx" => ("TX", 0x5b9bd5),
        "json" | "jsonc" | "json5" => ("{}", 0xe8c547),
        "toml" => ("TM", 0xc08adf),
        "yaml" | "yml" => ("YM", 0xcb6b6b),
        "md" | "markdown" | "mdx" => ("MD", 0x6bb5e0),
        "html" | "htm" => ("<>", 0xe8734a),
        "xml" | "svg" | "plist" | "csproj" | "xaml" => ("<>", 0xe8a85a),
        "css" | "scss" | "sass" | "less" => ("#", 0x56a8f5),
        "sh" | "bash" | "zsh" | "fish" | "ps1" | "bat" | "cmd" => ("$", 0x89d185),
        "go" => ("GO", 0x6ad7e5),
        "java" | "kt" | "kts" => ("JV", 0xe76f51),
        "c" | "h" => ("C", 0x6a9fd6),
        "cc" | "cpp" | "cxx" | "hpp" | "hh" => ("C+", 0x6a9fd6),
        "cs" => ("C#", 0x8c6bd6),
        "php" => ("PH", 0x8892bf),
        "rb" => ("RB", 0xe05a5a),
        "lua" => ("LU", 0x5a7fd6),
        "sql" => ("SQ", 0xd6a85a),
        "lock" => ("LK", 0x7a8290),
        "ini" | "cfg" | "conf" | "properties" => ("IN", 0x9aa1ad),
        "txt" | "log" => ("TX", 0x9aa1ad),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "ico" | "bmp" => ("IM", 0x9ccc65),
        "zip" | "gz" | "tar" | "7z" | "vsix" => ("ZP", 0xb59a6a),
        _ => ("··", 0x7a8290),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extensions_map_to_their_badges_case_insensitively() {
        assert_eq!(badge("main.rs").0, "RS");
        assert_eq!(badge("App.PY").0, "PY");
        assert_eq!(badge("index.tsx").0, "TX");
        assert_eq!(badge("package.json").0, "{}");
        assert_eq!(badge("styles.scss").0, "#");
    }

    #[test]
    fn well_known_names_beat_their_extension() {
        assert_eq!(badge("Dockerfile").0, "DK");
        assert_eq!(badge("Makefile").0, "MK");
        assert_eq!(badge(".gitignore").0, "GI");
        assert_eq!(badge(".env").0, "EN");
        assert_eq!(badge(".env.example").0, "EN");
        assert_eq!(badge("prod.env").0, "EN");
        assert_eq!(badge("Cargo.lock").0, "LK");
        assert_eq!(badge("README.md").0, "MD");
    }

    #[test]
    fn unknown_files_get_a_neutral_badge() {
        assert_eq!(badge("data.xyz").0, "··");
        assert_eq!(badge("noextension").0, "··");
    }
}
