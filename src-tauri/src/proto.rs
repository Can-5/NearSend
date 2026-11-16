// NearSend wire protocol: UDP presence beacon + TCP file transfer.
// All parsing/sanitizing logic lives here, unit-tested. No network code.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const UDP_PORT: u16 = 44123;
pub const TCP_PORT: u16 = 44124;
pub const MAGIC: &str = "nearsend-v1";
pub const HEADER_MAX: usize = 64 * 1024;
pub const MAX_FILES: usize = 50;
pub const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024 * 1024; // 2 GiB per file
pub const MAX_TOTAL_BYTES: u64 = 4 * 1024 * 1024 * 1024; // 4 GiB per transfer

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileSpec {
    pub name: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Header {
    pub magic: String,
    pub device: String,
    pub files: Vec<FileSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Presence {
    pub magic: String,
    pub device: String,
    pub port: u16,
}

/// Basename-only validation. Anything with a path separator, traversal, control
/// characters, trailing dot/space (Windows) or > 240 chars is rejected outright.
pub fn sanitize_file_name(name: &str) -> Result<String, String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("file name is empty".into());
    }
    if trimmed == "." || trimmed == ".." {
        return Err("file name may not be . or ..".into());
    }
    if trimmed.len() > 240 {
        return Err("file name exceeds 240 characters".into());
    }
    if trimmed.contains(['/', '\\', '\0']) {
        return Err("file name may not contain path separators".into());
    }
    if trimmed.chars().any(|c| c.is_control()) {
        return Err("file name may not contain control characters".into());
    }
    if trimmed.ends_with('.') || trimmed.ends_with(' ') {
        return Err("file name may not end with a dot or space".into());
    }
    Ok(trimmed.to_string())
}

/// Strips control characters for safe display (defense in depth for peer names).
pub fn sanitize_display_name(name: &str, max: usize) -> String {
    name.chars().filter(|c| !c.is_control()).take(max).collect::<String>().trim().to_string()
}

/// Parse + fully validate a transfer header line. Filenames come back sanitized.
pub fn parse_header_line(line: &[u8]) -> Result<Header, String> {
    if line.len() > HEADER_MAX {
        return Err("header too large".into());
    }
    let header: Header =
        serde_json::from_slice(line).map_err(|e| format!("invalid header JSON: {e}"))?;
    if header.magic != MAGIC {
        return Err("not a NearSend v1 peer".into());
    }
    if header.files.is_empty() {
        return Err("transfer contains no files".into());
    }
    if header.files.len() > MAX_FILES {
        return Err(format!("too many files (max {MAX_FILES})"));
    }
    let mut total: u64 = 0;
    let mut out = Vec::with_capacity(header.files.len());
    for f in &header.files {
        if f.size > MAX_FILE_BYTES {
            return Err(format!("file too large: {}", f.name));
        }
        total = total.saturating_add(f.size);
        if total > MAX_TOTAL_BYTES {
            return Err("transfer too large".into());
        }
        if f.sha256.len() != 64 || !f.sha256.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(format!("invalid sha256 for {}", f.name));
        }
        let name = sanitize_file_name(&f.name)?;
        out.push(FileSpec { name, size: f.size, sha256: f.sha256.to_lowercase() });
    }
    Ok(Header {
        magic: MAGIC.to_string(),
        device: sanitize_display_name(&header.device, 64),
        files: out,
    })
}

/// Parse a UDP presence beacon. Returns (device, advertised tcp port).
pub fn parse_presence(data: &[u8]) -> Option<(String, u16)> {
    let p: Presence = serde_json::from_slice(data).ok()?;
    if p.magic != MAGIC {
        return None;
    }
    let device = sanitize_display_name(&p.device, 64);
    if device.is_empty() {
        return None;
    }
    Some((device, p.port))
}

/// Candidate save paths: name, then name-1.ext, name-2.ext … (cap 100).
pub fn unique_path(dir: &Path, name: &str) -> Result<PathBuf, String> {
    let first = dir.join(name);
    if !first.exists() {
        return Ok(first);
    }
    let path = Path::new(name);
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or(name);
    let ext = path.extension().and_then(|s| s.to_str()).map(|e| format!(".{e}")).unwrap_or_default();
    for i in 1..=100 {
        let candidate = dir.join(format!("{stem}-{i}{ext}"));
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    Err("too many copies of this file exist".into())
}

/// Parse `nearsend://host:port` invite codes. Returns the authority unchanged.
pub fn parse_invite_code(code: &str) -> Result<String, String> {
    let trimmed = code.trim();
    let rest = trimmed
        .strip_prefix("nearsend://")
        .ok_or("invite code must start with nearsend://")?;
    let host = rest.trim_end_matches('/');
    if host.is_empty() || host.len() > 120 {
        return Err("invite code is empty or too long".into());
    }
    let (h, p) = host
        .rsplit_once(':')
        .ok_or("invite code must include a port (nearsend://192.168.1.5:44124)")?;
    let port: u16 = p.parse().map_err(|_| "invalid port in invite code")?;
    let valid = !h.is_empty()
        && h.len() <= 253
        && h.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == ':');
    if !valid {
        return Err("invalid host in invite code".into());
    }
    Ok(format!("{h}:{port}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_rejects_traversal_and_separators() {
        assert!(sanitize_file_name("../evil.txt").is_err());
        assert!(sanitize_file_name("a/b.txt").is_err());
        assert!(sanitize_file_name("a\\b.txt").is_err());
        assert!(sanitize_file_name("..").is_err());
        assert!(sanitize_file_name("CON").is_ok()); // name identical to a Windows device is fine as a file
        assert!(sanitize_file_name("photo (1).jpg").is_ok());
        assert!(sanitize_file_name("trailing.").is_err());
        assert_eq!(sanitize_file_name("trailing ").unwrap(), "trailing"); // outer whitespace is trimmed
        assert!(sanitize_file_name("").is_err());
    }

    #[test]
    fn header_parse_validates_everything() {
        let good = format!(
            r#"{{"magic":"nearsend-v1","device":"Desk","files":[{{"name":"a.txt","size":3,"sha256":"{}"}}]}}"#,
            "ab".repeat(32)
        );
        let h = parse_header_line(good.as_bytes()).unwrap();
        assert_eq!(h.files[0].name, "a.txt");
        assert_eq!(h.device, "Desk");
        assert!(parse_header_line(br#"{"magic":"nope","device":"x","files":[{"name":"a","size":1,"sha256":"abcd"}]}"#).is_err());
        assert!(parse_header_line(br#"{"magic":"nearsend-v1","device":"x","files":[]}"#).is_err());
    }

    #[test]
    fn header_rejects_bad_sha_and_oversized() {
        let big = format!(
            r#"{{"magic":"nearsend-v1","device":"d","files":[{{"name":"a","size":{},"sha256":"{}"}}]}}"#,
            MAX_FILE_BYTES + 1,
            "ab".repeat(32)
        );
        assert!(parse_header_line(big.as_bytes()).is_err());
        let badsha = format!(
            r#"{{"magic":"nearsend-v1","device":"d","files":[{{"name":"a","size":1,"sha256":"zz"}}]}}"#
        );
        assert!(parse_header_line(badsha.as_bytes()).is_err());
    }

    #[test]
    fn unique_path_avoids_collisions() {
        let dir = std::env::temp_dir().join(format!("nearsend-up-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "x").unwrap();
        std::fs::write(dir.join("a-1.txt"), "x").unwrap();
        let p = unique_path(&dir, "a.txt").unwrap();
        assert_eq!(p.file_name().unwrap().to_str().unwrap(), "a-2.txt");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn invite_code_roundtrip() {
        assert_eq!(parse_invite_code("nearsend://192.168.1.5:44124").unwrap(), "192.168.1.5:44124");
        assert!(parse_invite_code("https://evil.com").is_err());
        assert!(parse_invite_code("nearsend://host").is_err());
        assert!(parse_invite_code("nearsend://host:aa").is_err());
    }

    #[test]
    fn presence_parses() {
        let good = br#"{"magic":"nearsend-v1","device":"Kitchen PC","port":44124}"#;
        assert_eq!(parse_presence(good).unwrap().0, "Kitchen PC");
        assert!(parse_presence(br#"{"magic":"other","device":"x","port":1}"#).is_none());
    }
}
