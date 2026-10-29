//! A small Open VSX client: search, look up an extension, download a `.vsix`.
//!
//! Open VSX (open-vsx.org) is the registry Forge uses. Microsoft's Marketplace terms limit
//! it to Microsoft's own products. HTTP goes through `curl`, which Windows 10+ ships, the
//! same tool the language-server installer uses. Everything the server returns is data:
//! download URLs are only followed if they stay on open-vsx.org over https.

use std::path::Path;
use std::process::{Command, Stdio};

use serde_json::Value;
use sha2::{Digest, Sha256};

const API: &str = "https://open-vsx.org/api";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listing {
    pub namespace: String,
    pub name: String,
    pub version: String,
    pub display_name: String,
    pub description: String,
    pub downloads: u64,
    pub download_url: String,
    pub sha256_url: Option<String>,
}

impl Listing {
    pub fn id(&self) -> String {
        format!("{}.{}", self.namespace, self.name)
    }
}

/// Whether `url` may be fetched: https and exactly the Open VSX host.
pub fn is_trusted_url(url: &str) -> bool {
    url.strip_prefix("https://open-vsx.org/").is_some_and(|rest| !rest.starts_with('/') && !url.contains('@') && !url.contains(['\\', ' ']))
}

fn percent_encode(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(byte as char),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn curl_text(url: &str) -> Result<String, String> {
    let mut command = Command::new("curl");
    command.args(["-sS", "-L", "-f", "-m", "30", "--max-filesize", "8000000", "-A", "forge-editor"]).arg(url);
    run(command).map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
}

fn run(mut command: Command) -> Result<Vec<u8>, String> {
    command.stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let output = command.output().map_err(|e| format!("could not run curl: {e}"))?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        let error = String::from_utf8_lossy(&output.stderr);
        Err(error.lines().next().unwrap_or("request failed").trim().to_owned())
    }
}

/// Parse a search response (or a single-extension response wrapped by [`lookup`]).
pub fn parse_listings(json: &str) -> Result<Vec<Listing>, String> {
    let value: Value = serde_json::from_str(json).map_err(|e| format!("unexpected response: {e}"))?;
    if let Some(error) = value.get("error").and_then(Value::as_str) {
        return Err(error.to_owned());
    }
    let items = match value.get("extensions").and_then(Value::as_array) {
        Some(items) => items.clone(),
        None => vec![value],
    };
    Ok(items.iter().filter_map(parse_listing).collect())
}

fn parse_listing(item: &Value) -> Option<Listing> {
    let text = |key: &str| item.get(key).and_then(Value::as_str).map(str::to_owned);
    let files = item.get("files")?;
    let download_url = files.get("download")?.as_str()?.to_owned();
    if !is_trusted_url(&download_url) {
        return None;
    }
    let sha256_url = files.get("sha256").and_then(Value::as_str).filter(|u| is_trusted_url(u)).map(str::to_owned);
    let name = text("name")?;
    Some(Listing {
        namespace: text("namespace")?,
        display_name: text("displayName").filter(|d| !d.trim().is_empty()).unwrap_or_else(|| name.clone()),
        name,
        version: text("version").unwrap_or_default(),
        description: text("description").unwrap_or_default(),
        downloads: item.get("downloadCount").and_then(Value::as_u64).unwrap_or(0),
        download_url,
        sha256_url,
    })
}

pub fn search(query: &str, limit: usize) -> Result<Vec<Listing>, String> {
    let url = format!("{API}/-/search?query={}&size={limit}&sortBy=relevance", percent_encode(query.trim()));
    parse_listings(&curl_text(&url)?)
}

/// Look up one extension by `publisher.name`.
pub fn lookup(id: &str) -> Result<Listing, String> {
    let (namespace, name) = id.split_once('.').ok_or("an extension id looks like publisher.name")?;
    if namespace.is_empty() || name.is_empty() || id.contains(['/', '\\', ' ']) {
        return Err("an extension id looks like publisher.name".to_owned());
    }
    let url = format!("{API}/{}/{}", percent_encode(namespace), percent_encode(name));
    parse_listings(&curl_text(&url)?)?.into_iter().next().ok_or_else(|| format!("{id} was not found on Open VSX"))
}

/// What Open VSX knows about one extension beyond its search listing.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Details {
    pub license: Option<String>,
    pub repository: Option<String>,
    pub homepage: Option<String>,
    pub categories: Vec<String>,
    pub rating: Option<f64>,
    pub rating_count: u64,
    pub downloads: u64,
    /// The extension's README (Markdown), when it has one.
    pub readme: Option<String>,
}

/// Parse a single-extension response. The second value is where its README can be fetched.
pub fn parse_details(json: &str) -> Result<(Details, Option<String>), String> {
    let value: Value = serde_json::from_str(json).map_err(|e| format!("unexpected response: {e}"))?;
    if let Some(error) = value.get("error").and_then(Value::as_str) {
        return Err(error.to_owned());
    }
    let text = |key: &str| value.get(key).and_then(Value::as_str).filter(|s| !s.trim().is_empty()).map(str::to_owned);
    let readme_url = value.get("files").and_then(|f| f.get("readme")).and_then(Value::as_str).filter(|u| is_trusted_url(u)).map(str::to_owned);
    let details = Details {
        license: text("license"),
        repository: text("repository"),
        homepage: text("homepage"),
        categories: value.get("categories").and_then(Value::as_array).map(|a| a.iter().filter_map(|c| c.as_str().map(str::to_owned)).collect()).unwrap_or_default(),
        rating: value.get("averageRating").and_then(Value::as_f64),
        rating_count: value.get("reviewCount").and_then(Value::as_u64).unwrap_or(0),
        downloads: value.get("downloadCount").and_then(Value::as_u64).unwrap_or(0),
        readme: None,
    };
    Ok((details, readme_url))
}

/// Everything shown on an extension's page: its details and README. Blocks; run on a background thread.
pub fn details(id: &str) -> Result<Details, String> {
    let (namespace, name) = id.split_once('.').ok_or("an extension id looks like publisher.name")?;
    if namespace.is_empty() || name.is_empty() || id.contains(['/', '\\', ' ']) {
        return Err("an extension id looks like publisher.name".to_owned());
    }
    let url = format!("{API}/{}/{}", percent_encode(namespace), percent_encode(name));
    let (mut details, readme_url) = parse_details(&curl_text(&url)?)?;
    if let Some(url) = readme_url {
        details.readme = curl_text(&url).ok();
    }
    Ok(details)
}

/// The expected digest from a `.sha256` file: its first token, lowercase.
pub fn parse_digest(text: &str) -> Option<String> {
    let token = text.split_whitespace().next()?.to_ascii_lowercase();
    (token.len() == 64 && token.bytes().all(|b| b.is_ascii_hexdigit())).then_some(token)
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// Download the `.vsix` of `listing` to `destination`, checking its SHA-256 when the
/// registry publishes one. A file that fails the check is deleted.
pub fn download(listing: &Listing, destination: &Path) -> Result<(), String> {
    if !is_trusted_url(&listing.download_url) {
        return Err("refusing to download from outside open-vsx.org".to_owned());
    }
    let mut command = Command::new("curl");
    command.args(["-sS", "-L", "-f", "-m", "300", "--max-filesize", "300000000", "-A", "forge-editor", "-o"]).arg(destination).arg(&listing.download_url);
    if let Err(error) = run(command) {
        let _ = std::fs::remove_file(destination);
        return Err(error);
    }
    if let Some(url) = &listing.sha256_url {
        let expected = curl_text(url).ok().and_then(|t| parse_digest(&t));
        if let Some(expected) = expected {
            let actual = sha256_hex(&std::fs::read(destination).map_err(|e| e.to_string())?);
            if actual != expected {
                let _ = std::fs::remove_file(destination);
                return Err(format!("the download does not match its published checksum ({actual} vs {expected})"));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const RESPONSE: &str = r#"{ "offset": 0, "totalSize": 3, "extensions": [
        { "name": "night", "namespace": "acme", "version": "1.2.0", "displayName": "Night Owl", "description": "A theme", "downloadCount": 42,
          "files": { "download": "https://open-vsx.org/api/acme/night/1.2.0/file/acme.night-1.2.0.vsix", "sha256": "https://open-vsx.org/api/acme/night/1.2.0/file/acme.night-1.2.0.sha256" } },
        { "name": "evil", "namespace": "x", "version": "1", "files": { "download": "https://evil.example/x.vsix" } },
        { "name": "sneaky", "namespace": "x", "version": "1", "files": { "download": "https://open-vsx.org@evil.example/x.vsix" } },
        { "name": "plain", "namespace": "acme", "files": { "download": "https://open-vsx.org/api/acme/plain/file/p.vsix", "sha256": "https://evil.example/p.sha256" } }
    ] }"#;

    #[test]
    fn details_parse_and_only_trusted_readme_urls_are_kept() {
        let json = r#"{ "license": "MIT", "repository": "https://github.com/a/b", "categories": ["Themes"], "averageRating": 4.5, "reviewCount": 12, "downloadCount": 99,
            "files": { "readme": "https://open-vsx.org/api/a/b/1/file/README.md" } }"#;
        let (details, readme) = parse_details(json).unwrap();
        assert_eq!(details.license.as_deref(), Some("MIT"));
        assert_eq!((details.rating, details.rating_count, details.downloads), (Some(4.5), 12, 99));
        assert_eq!(details.categories, ["Themes"]);
        assert_eq!(readme.as_deref(), Some("https://open-vsx.org/api/a/b/1/file/README.md"));
        let (_, evil) = parse_details(r#"{ "files": { "readme": "https://evil.example/README.md" } }"#).unwrap();
        assert_eq!(evil, None);
        assert!(parse_details(r#"{ "error": "not found" }"#).is_err());
    }

    #[test]
    fn search_results_parse_and_untrusted_hosts_are_dropped() {
        let listings = parse_listings(RESPONSE).unwrap();
        assert_eq!(listings.len(), 2, "{listings:#?}");
        assert_eq!(listings[0].id(), "acme.night");
        assert_eq!(listings[0].display_name, "Night Owl");
        assert_eq!(listings[0].downloads, 42);
        assert!(listings[0].sha256_url.is_some());
        // A checksum URL on another host is ignored, the extension itself is kept.
        assert_eq!(listings[1].display_name, "plain");
        assert_eq!(listings[1].sha256_url, None);
    }

    #[test]
    fn a_single_extension_response_and_errors_are_understood() {
        let single = r#"{ "name": "a", "namespace": "b", "version": "1", "files": { "download": "https://open-vsx.org/api/b/a/1/file/b.a-1.vsix" } }"#;
        assert_eq!(parse_listings(single).unwrap()[0].id(), "b.a");
        assert_eq!(parse_listings(r#"{ "error": "Extension not found" }"#).unwrap_err(), "Extension not found");
        assert!(parse_listings("<html>").is_err());
    }

    #[test]
    fn only_https_open_vsx_urls_are_trusted() {
        assert!(is_trusted_url("https://open-vsx.org/api/a/b"));
        for url in ["http://open-vsx.org/api/a", "https://open-vsx.org.evil.example/x", "https://evil.example/open-vsx.org/", "https://open-vsx.org@evil.example/", "https://open-vsx.org//evil", "file:///c:/x", "https://open-vsx.org/a b"] {
            assert!(!is_trusted_url(url), "{url}");
        }
    }

    #[test]
    fn digests_are_parsed_and_computed() {
        let known = sha256_hex(b"abc");
        assert_eq!(known, "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert_eq!(parse_digest(&format!("{}  file.vsix\n", known.to_uppercase())).as_deref(), Some(known.as_str()));
        assert_eq!(parse_digest("not a digest"), None);
        assert_eq!(percent_encode("dark theme+c#"), "dark%20theme%2Bc%23");
    }
}
