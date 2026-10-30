use std::path::Path;

/// Every language Forge recognises. Most have a tree-sitter grammar; a few (`.env`,
/// INI, Dockerfile, Makefile, ignore files) are highlighted by small line-based rules.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Language {
    Rust,
    Json,
    JavaScript,
    JavaScriptReact,
    TypeScript,
    TypeScriptReact,
    Python,
    Toml,
    Html,
    Css,
    Xml,
    Yaml,
    Markdown,
    Shell,
    Go,
    C,
    Cpp,
    Java,
    CSharp,
    Php,
    Ruby,
    Lua,
    Sql,
    Diff,
    Env,
    Ini,
    Dockerfile,
    Makefile,
    Ignore,
}

/// How a language is highlighted.
pub(crate) enum Grammar {
    /// A tree-sitter language and its highlight queries, most complete first; the first
    /// one that compiles is used.
    Tree(tree_sitter::Language, Vec<String>),
    /// Per-line rules (see `lines.rs`).
    Lines,
}

impl Language {
    pub const ALL: &'static [Language] = &[
        Language::Rust,
        Language::Json,
        Language::JavaScript,
        Language::JavaScriptReact,
        Language::TypeScript,
        Language::TypeScriptReact,
        Language::Python,
        Language::Toml,
        Language::Html,
        Language::Css,
        Language::Xml,
        Language::Yaml,
        Language::Markdown,
        Language::Shell,
        Language::Go,
        Language::C,
        Language::Cpp,
        Language::Java,
        Language::CSharp,
        Language::Php,
        Language::Ruby,
        Language::Lua,
        Language::Sql,
        Language::Diff,
        Language::Env,
        Language::Ini,
        Language::Dockerfile,
        Language::Makefile,
        Language::Ignore,
    ];

    /// Detect a language from a path: well-known file names first (`.env`, `Dockerfile`,
    /// `.bashrc`), then the extension.
    pub fn from_path(path: &Path) -> Option<Self> {
        let file = path.file_name()?.to_str()?.to_ascii_lowercase();
        if let Some(language) = Self::from_file_name(&file) {
            return Some(language);
        }
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        Some(match ext.as_str() {
            "rs" => Self::Rust,
            "json" | "jsonc" | "json5" | "webmanifest" | "geojson" => Self::Json,
            "js" | "mjs" | "cjs" => Self::JavaScript,
            "jsx" => Self::JavaScriptReact,
            "ts" | "mts" | "cts" => Self::TypeScript,
            "tsx" => Self::TypeScriptReact,
            "py" | "pyw" | "pyi" => Self::Python,
            "toml" => Self::Toml,
            "html" | "htm" | "xhtml" | "vue" | "svelte" => Self::Html,
            // SCSS and Less are close enough to CSS for useful colors.
            "css" | "scss" | "less" => Self::Css,
            "xml" | "xsd" | "xsl" | "xslt" | "svg" | "xaml" | "plist" | "csproj" | "fsproj" | "vbproj" | "props" | "targets" | "resx"
            | "wsdl" | "rss" | "atom" | "dtd" | "xib" | "storyboard" | "nuspec" | "manifest" | "kml" | "gpx" | "config" => Self::Xml,
            "yml" | "yaml" => Self::Yaml,
            "md" | "markdown" | "mdown" | "mkd" => Self::Markdown,
            "sh" | "bash" | "zsh" | "ksh" => Self::Shell,
            "go" => Self::Go,
            "c" | "h" => Self::C,
            "cpp" | "cc" | "cxx" | "hpp" | "hh" | "hxx" | "ino" | "inl" => Self::Cpp,
            "java" => Self::Java,
            "cs" | "csx" => Self::CSharp,
            "php" | "phtml" => Self::Php,
            "rb" | "rake" | "gemspec" | "ru" => Self::Ruby,
            "lua" => Self::Lua,
            "sql" => Self::Sql,
            "diff" | "patch" => Self::Diff,
            "env" => Self::Env,
            "ini" | "cfg" | "conf" | "properties" | "editorconfig" | "gitconfig" => Self::Ini,
            "dockerfile" => Self::Dockerfile,
            "mk" | "mak" => Self::Makefile,
            _ => return None,
        })
    }

    /// Well-known names that have no (useful) extension. `file` is lowercase.
    fn from_file_name(file: &str) -> Option<Self> {
        Some(match file {
            ".env" => Self::Env,
            f if f.starts_with(".env.") || f.ends_with(".env") => Self::Env,
            "dockerfile" => Self::Dockerfile,
            f if f.starts_with("dockerfile.") => Self::Dockerfile,
            "makefile" | "gnumakefile" => Self::Makefile,
            ".gitignore" | ".dockerignore" | ".npmignore" | ".eslintignore" | ".prettierignore" | ".gitattributes" | ".hgignore" => Self::Ignore,
            ".bashrc" | ".bash_profile" | ".bash_aliases" | ".zshrc" | ".zprofile" | ".profile" | ".zshenv" | "pkgbuild" => Self::Shell,
            "cargo.lock" | "poetry.lock" | "uv.lock" | "pipfile" => Self::Toml,
            ".babelrc" | ".eslintrc" | ".prettierrc" | ".swcrc" | "tsconfig.json" | "jsconfig.json" => Self::Json,
            "gemfile" | "rakefile" | "podfile" | "vagrantfile" | "guardfile" | "brewfile" => Self::Ruby,
            ".gitconfig" | ".editorconfig" | "setup.cfg" | "tox.ini" => Self::Ini,
            _ => return None,
        })
    }

    /// The language a Markdown fence or a `<script lang>` names ("rust", "ts", "py", "sh"...).
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name.to_ascii_lowercase().as_str() {
            "rust" | "rs" => Self::Rust,
            "json" | "jsonc" | "json5" => Self::Json,
            "javascript" | "js" | "mjs" | "cjs" | "node" => Self::JavaScript,
            "jsx" => Self::JavaScriptReact,
            "typescript" | "ts" => Self::TypeScript,
            "tsx" => Self::TypeScriptReact,
            "python" | "py" => Self::Python,
            "toml" => Self::Toml,
            "html" | "htm" => Self::Html,
            "css" | "scss" | "less" => Self::Css,
            "xml" | "svg" | "xsd" | "xslt" | "plist" => Self::Xml,
            "yaml" | "yml" => Self::Yaml,
            "markdown" | "md" => Self::Markdown,
            "bash" | "sh" | "shell" | "zsh" | "console" | "shellscript" => Self::Shell,
            "go" | "golang" => Self::Go,
            "c" => Self::C,
            "cpp" | "c++" | "cc" | "cxx" => Self::Cpp,
            "java" => Self::Java,
            "csharp" | "cs" | "c#" => Self::CSharp,
            "php" => Self::Php,
            "ruby" | "rb" => Self::Ruby,
            "lua" => Self::Lua,
            "sql" => Self::Sql,
            "diff" | "patch" => Self::Diff,
            "env" | "dotenv" => Self::Env,
            "ini" | "conf" | "properties" => Self::Ini,
            "dockerfile" | "docker" => Self::Dockerfile,
            "makefile" | "make" => Self::Makefile,
            _ => return None,
        })
    }

    /// Highlighted line by line instead of by a syntax tree.
    pub fn is_line_based(self) -> bool {
        matches!(self, Self::Env | Self::Ini | Self::Dockerfile | Self::Makefile | Self::Ignore | Self::Diff)
    }

    /// The id VS Code uses for this language (what a server expects in `didOpen`).
    pub fn language_id(self) -> &'static str {
        match self {
            Self::Rust => "rust",
            Self::Json => "json",
            Self::JavaScript => "javascript",
            Self::JavaScriptReact => "javascriptreact",
            Self::TypeScript => "typescript",
            Self::TypeScriptReact => "typescriptreact",
            Self::Python => "python",
            Self::Toml => "toml",
            Self::Html => "html",
            Self::Css => "css",
            Self::Xml => "xml",
            Self::Yaml => "yaml",
            Self::Markdown => "markdown",
            Self::Shell => "shellscript",
            Self::Go => "go",
            Self::C => "c",
            Self::Cpp => "cpp",
            Self::Java => "java",
            Self::CSharp => "csharp",
            Self::Php => "php",
            Self::Ruby => "ruby",
            Self::Lua => "lua",
            Self::Sql => "sql",
            Self::Diff => "diff",
            Self::Env => "dotenv",
            Self::Ini => "ini",
            Self::Dockerfile => "dockerfile",
            Self::Makefile => "makefile",
            Self::Ignore => "ignore",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Rust => "Rust",
            Self::Json => "JSON",
            Self::JavaScript => "JavaScript",
            Self::JavaScriptReact => "JavaScript React",
            Self::TypeScript => "TypeScript",
            Self::TypeScriptReact => "TypeScript React",
            Self::Python => "Python",
            Self::Toml => "TOML",
            Self::Html => "HTML",
            Self::Css => "CSS",
            Self::Xml => "XML",
            Self::Yaml => "YAML",
            Self::Markdown => "Markdown",
            Self::Shell => "Shell Script",
            Self::Go => "Go",
            Self::C => "C",
            Self::Cpp => "C++",
            Self::Java => "Java",
            Self::CSharp => "C#",
            Self::Php => "PHP",
            Self::Ruby => "Ruby",
            Self::Lua => "Lua",
            Self::Sql => "SQL",
            Self::Diff => "Diff",
            Self::Env => "Dotenv",
            Self::Ini => "Properties",
            Self::Dockerfile => "Dockerfile",
            Self::Makefile => "Makefile",
            Self::Ignore => "Ignore",
        }
    }

    /// The id VS Code gives this language, used in settings such as `"[rust]": { ... }`.
    pub fn vscode_id(self) -> &'static str {
        match self {
            Self::Rust => "rust",
            Self::Json => "json",
            Self::JavaScript => "javascript",
            Self::JavaScriptReact => "javascriptreact",
            Self::TypeScript => "typescript",
            Self::TypeScriptReact => "typescriptreact",
            Self::Python => "python",
            Self::Toml => "toml",
            Self::Html => "html",
            Self::Css => "css",
            Self::Xml => "xml",
            Self::Yaml => "yaml",
            Self::Markdown => "markdown",
            Self::Shell => "shellscript",
            Self::Go => "go",
            Self::C => "c",
            Self::Cpp => "cpp",
            Self::Java => "java",
            Self::CSharp => "csharp",
            Self::Php => "php",
            Self::Ruby => "ruby",
            Self::Lua => "lua",
            Self::Sql => "sql",
            Self::Diff => "diff",
            Self::Env => "dotenv",
            Self::Ini => "properties",
            Self::Dockerfile => "dockerfile",
            Self::Makefile => "makefile",
            Self::Ignore => "ignore",
        }
    }

    /// Token that starts a line comment, for Ctrl+/.
    pub fn line_comment(self) -> Option<&'static str> {
        match self {
            Self::Rust | Self::JavaScript | Self::JavaScriptReact | Self::TypeScript | Self::TypeScriptReact | Self::Go | Self::C | Self::Cpp
            | Self::Java | Self::CSharp | Self::Php => Some("//"),
            Self::Python | Self::Toml | Self::Yaml | Self::Shell | Self::Ruby | Self::Env | Self::Dockerfile | Self::Makefile | Self::Ignore => Some("#"),
            Self::Lua | Self::Sql => Some("--"),
            Self::Ini => Some(";"),
            Self::Json | Self::Html | Self::Css | Self::Xml | Self::Markdown | Self::Diff => None,
        }
    }

    /// Open and close tokens of a block comment, for Shift+Alt+A.
    pub fn block_comment(self) -> Option<(&'static str, &'static str)> {
        match self {
            Self::Rust | Self::JavaScript | Self::JavaScriptReact | Self::TypeScript | Self::TypeScriptReact | Self::Go | Self::C | Self::Cpp
            | Self::Java | Self::CSharp | Self::Php | Self::Css | Self::Sql => Some(("/*", "*/")),
            Self::Html | Self::Xml | Self::Markdown => Some(("<!--", "-->")),
            Self::Lua => Some(("--[[", "]]")),
            _ => None,
        }
    }

    /// Whether a line ending in `:` indents the next one.
    pub fn indents_after_colon(self) -> bool {
        matches!(self, Self::Python | Self::Yaml)
    }

    pub(crate) fn grammar(self) -> Grammar {
        let q = |parts: &[&str]| vec![parts.concat()];
        match self {
            Self::Rust => Grammar::Tree(tree_sitter_rust::LANGUAGE.into(), q(&[tree_sitter_rust::HIGHLIGHTS_QUERY])),
            Self::Json => Grammar::Tree(tree_sitter_json::LANGUAGE.into(), q(&[tree_sitter_json::HIGHLIGHTS_QUERY])),
            Self::JavaScript | Self::JavaScriptReact => Grammar::Tree(
                tree_sitter_javascript::LANGUAGE.into(),
                vec![
                    [tree_sitter_javascript::HIGHLIGHT_QUERY, tree_sitter_javascript::JSX_HIGHLIGHT_QUERY].join("\n"),
                    tree_sitter_javascript::HIGHLIGHT_QUERY.to_owned(),
                ],
            ),
            // The TypeScript query only adds to the JavaScript one.
            Self::TypeScript => Grammar::Tree(
                tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
                vec![[tree_sitter_javascript::HIGHLIGHT_QUERY, tree_sitter_typescript::HIGHLIGHTS_QUERY].join("\n"), tree_sitter_typescript::HIGHLIGHTS_QUERY.to_owned()],
            ),
            Self::TypeScriptReact => Grammar::Tree(
                tree_sitter_typescript::LANGUAGE_TSX.into(),
                vec![
                    [tree_sitter_javascript::HIGHLIGHT_QUERY, tree_sitter_typescript::HIGHLIGHTS_QUERY, tree_sitter_javascript::JSX_HIGHLIGHT_QUERY].join("\n"),
                    [tree_sitter_javascript::HIGHLIGHT_QUERY, tree_sitter_typescript::HIGHLIGHTS_QUERY].join("\n"),
                    tree_sitter_typescript::HIGHLIGHTS_QUERY.to_owned(),
                ],
            ),
            Self::Python => Grammar::Tree(tree_sitter_python::LANGUAGE.into(), q(&[tree_sitter_python::HIGHLIGHTS_QUERY])),
            Self::Toml => Grammar::Tree(tree_sitter_toml_ng::LANGUAGE.into(), q(&[tree_sitter_toml_ng::HIGHLIGHTS_QUERY])),
            Self::Html => Grammar::Tree(tree_sitter_html::LANGUAGE.into(), q(&[tree_sitter_html::HIGHLIGHTS_QUERY])),
            Self::Css => Grammar::Tree(tree_sitter_css::LANGUAGE.into(), q(&[tree_sitter_css::HIGHLIGHTS_QUERY])),
            Self::Xml => Grammar::Tree(tree_sitter_xml::LANGUAGE_XML.into(), q(&[tree_sitter_xml::XML_HIGHLIGHT_QUERY])),
            Self::Yaml => Grammar::Tree(tree_sitter_yaml::LANGUAGE.into(), q(&[tree_sitter_yaml::HIGHLIGHTS_QUERY])),
            Self::Markdown => Grammar::Tree(tree_sitter_md::LANGUAGE.into(), q(&[tree_sitter_md::HIGHLIGHT_QUERY_BLOCK])),
            Self::Shell => Grammar::Tree(tree_sitter_bash::LANGUAGE.into(), q(&[tree_sitter_bash::HIGHLIGHT_QUERY])),
            Self::Go => Grammar::Tree(tree_sitter_go::LANGUAGE.into(), q(&[tree_sitter_go::HIGHLIGHTS_QUERY])),
            Self::C => Grammar::Tree(tree_sitter_c::LANGUAGE.into(), q(&[tree_sitter_c::HIGHLIGHT_QUERY])),
            // The C++ query only adds to the C one.
            Self::Cpp => Grammar::Tree(
                tree_sitter_cpp::LANGUAGE.into(),
                vec![[tree_sitter_c::HIGHLIGHT_QUERY, tree_sitter_cpp::HIGHLIGHT_QUERY].join("\n"), tree_sitter_cpp::HIGHLIGHT_QUERY.to_owned()],
            ),
            Self::Java => Grammar::Tree(tree_sitter_java::LANGUAGE.into(), q(&[tree_sitter_java::HIGHLIGHTS_QUERY])),
            Self::CSharp => Grammar::Tree(tree_sitter_c_sharp::LANGUAGE.into(), q(&[tree_sitter_c_sharp::HIGHLIGHTS_QUERY])),
            Self::Php => Grammar::Tree(tree_sitter_php::LANGUAGE_PHP.into(), q(&[tree_sitter_php::HIGHLIGHTS_QUERY])),
            Self::Ruby => Grammar::Tree(tree_sitter_ruby::LANGUAGE.into(), q(&[tree_sitter_ruby::HIGHLIGHTS_QUERY])),
            Self::Lua => Grammar::Tree(tree_sitter_lua::LANGUAGE.into(), q(&[tree_sitter_lua::HIGHLIGHTS_QUERY])),
            Self::Sql => Grammar::Tree(tree_sitter_sequel::LANGUAGE.into(), q(&[tree_sitter_sequel::HIGHLIGHTS_QUERY])),
            Self::Env | Self::Ini | Self::Dockerfile | Self::Makefile | Self::Ignore | Self::Diff => Grammar::Lines,
        }
    }
}

/// Coarse classes a theme assigns colors to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HighlightClass {
    Keyword,
    Function,
    Type,
    String,
    Escape,
    Number,
    Constant,
    Comment,
    Property,
    Attribute,
    Operator,
    Punctuation,
    Variable,
    Tag,
}

impl HighlightClass {
    /// Map a query capture name (`keyword.control`, `function.macro`, ...) to a class,
    /// by its leading segment with a few specific overrides. Unknown names are ignored.
    pub fn from_capture(name: &str) -> Option<Self> {
        match name {
            "variable.builtin" | "variable.special" | "variable.language" => return Some(Self::Keyword),
            "string.escape" | "escape" | "string.special" | "string.special.symbol" => return Some(Self::Escape),
            "constant.numeric" => return Some(Self::Number),
            "function.macro" => return Some(Self::Function),
            "constructor" => return Some(Self::Type),
            // Markdown.
            "text.title" | "markup.heading" => return Some(Self::Type),
            "text.literal" | "markup.raw" | "markup.raw.block" => return Some(Self::String),
            "text.uri" | "text.reference" | "markup.link" | "markup.link.url" => return Some(Self::Constant),
            "text.emphasis" | "markup.italic" | "text.strong" | "markup.bold" => return Some(Self::Variable),
            // Diff.
            "diff.plus" | "diff.added" => return Some(Self::String),
            "diff.minus" | "diff.deleted" => return Some(Self::Tag),
            "diff.delta" | "diff.changed" => return Some(Self::Constant),
            // Tags and attributes in HTML / XML / JSX.
            "tag.delimiter" | "punctuation.delimiter" => return Some(Self::Punctuation),
            "tag.attribute" => return Some(Self::Attribute),
            _ => {}
        }
        let head = name.split('.').next().unwrap_or(name);
        Some(match head {
            "keyword" | "conditional" | "repeat" | "include" | "exception" | "storage" | "preproc" | "define" => Self::Keyword,
            "function" | "method" => Self::Function,
            "type" | "namespace" | "module" | "class" | "interface" | "enum" | "struct" => Self::Type,
            "string" | "character" | "text" => Self::String,
            "number" | "float" => Self::Number,
            "constant" | "boolean" | "symbol" => Self::Constant,
            "comment" => Self::Comment,
            "property" | "field" | "label" | "key" => Self::Property,
            "attribute" => Self::Attribute,
            "operator" => Self::Operator,
            "punctuation" | "delimiter" => Self::Punctuation,
            "variable" | "parameter" | "identifier" => Self::Variable,
            "tag" => Self::Tag,
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lang(path: &str) -> Option<Language> {
        Language::from_path(Path::new(path))
    }

    #[test]
    fn detects_language_by_extension() {
        assert_eq!(lang("src/main.RS"), Some(Language::Rust));
        assert_eq!(lang("a.jsx"), Some(Language::JavaScriptReact));
        assert_eq!(lang("a.tsx"), Some(Language::TypeScriptReact));
        assert_eq!(lang("a.ts"), Some(Language::TypeScript));
        assert_eq!(lang("a.mts"), Some(Language::TypeScript));
        assert_eq!(lang("page.html"), Some(Language::Html));
        assert_eq!(lang("pom.xml"), Some(Language::Xml));
        assert_eq!(lang("logo.svg"), Some(Language::Xml));
        assert_eq!(lang("App.csproj"), Some(Language::Xml));
        assert_eq!(lang("ci.yml"), Some(Language::Yaml));
        assert_eq!(lang("x.unknownext"), None);
        assert_eq!(lang("yarn.lock"), None, "a generic .lock file is not assumed to be TOML");
    }

    #[test]
    fn detects_well_known_file_names_with_no_useful_extension() {
        assert_eq!(lang(".env"), Some(Language::Env), "Rust sees `.env` as having no extension");
        assert_eq!(lang("app/.env.local"), Some(Language::Env));
        assert_eq!(lang(".env.production"), Some(Language::Env));
        assert_eq!(lang("prod.env"), Some(Language::Env));
        assert_eq!(lang("Dockerfile"), Some(Language::Dockerfile));
        assert_eq!(lang("Dockerfile.dev"), Some(Language::Dockerfile));
        assert_eq!(lang("Makefile"), Some(Language::Makefile));
        assert_eq!(lang(".gitignore"), Some(Language::Ignore));
        assert_eq!(lang("home/.bashrc"), Some(Language::Shell));
        assert_eq!(lang("Cargo.lock"), Some(Language::Toml));
        assert_eq!(lang("tsconfig.json"), Some(Language::Json));
        assert_eq!(lang("Gemfile"), Some(Language::Ruby));
    }

    #[test]
    fn every_language_has_a_name_id_and_consistent_comment_tokens() {
        let mut ids = std::collections::HashSet::new();
        for l in Language::ALL {
            assert!(!l.name().is_empty());
            assert!(ids.insert(l.language_id()), "duplicate language id {}", l.language_id());
            if let Some((open, close)) = l.block_comment() {
                assert!(!open.is_empty() && !close.is_empty());
            }
        }
        assert_eq!(Language::TypeScriptReact.language_id(), "typescriptreact");
        assert_eq!(Language::Shell.language_id(), "shellscript");
        assert_eq!(Language::Python.line_comment(), Some("#"));
        assert_eq!(Language::Html.block_comment(), Some(("<!--", "-->")));
        assert!(Language::Yaml.indents_after_colon());
    }

    #[test]
    fn capture_names_map_to_classes() {
        assert_eq!(HighlightClass::from_capture("keyword.control.return"), Some(HighlightClass::Keyword));
        assert_eq!(HighlightClass::from_capture("variable.builtin"), Some(HighlightClass::Keyword));
        assert_eq!(HighlightClass::from_capture("variable.parameter"), Some(HighlightClass::Variable));
        assert_eq!(HighlightClass::from_capture("function.method"), Some(HighlightClass::Function));
        assert_eq!(HighlightClass::from_capture("text.title"), Some(HighlightClass::Type));
        assert_eq!(HighlightClass::from_capture("diff.plus"), Some(HighlightClass::String));
        assert_eq!(HighlightClass::from_capture("tag.attribute"), Some(HighlightClass::Attribute));
        assert_eq!(HighlightClass::from_capture("embedded"), None);
        assert_eq!(HighlightClass::from_capture("spell"), None);
    }
}
