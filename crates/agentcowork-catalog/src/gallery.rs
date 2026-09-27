//! P52.2 — model gallery index: `gallery@model` entries with per-file
//! sha256 pins, an optional backend override, and a preload flag.
//!
//! The index ships as minimal YAML parsed here with zero new dependencies
//! (only the `id:` / `files:` / `path:` / `sha256:` subset below — anything
//! else is ignored for forward compatibility). File integrity reuses the
//! SHA-256 discipline of `agentcowork-core::models::hf::sha256_hex`, vendored
//! here as a dependency-free streaming implementation because this crate
//! deliberately does not depend on `sha2` (see `Cargo.toml`).

use serde::{Deserialize, Serialize};
use std::path::{Component, Path};

// ---------------------------------------------------------------------------
// Index shape
// ---------------------------------------------------------------------------

/// One pinned file inside a gallery entry.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GalleryFile {
    /// Path relative to the gallery dir (never absolute, never `..`).
    pub path: String,
    /// Expected hex sha256 of the file bytes.
    pub sha256: String,
}

/// One `gallery@model` entry.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GalleryEntry {
    /// `gallery@model` (see [`parse_id`]).
    pub id: String,
    pub files: Vec<GalleryFile>,
    /// Backend override (e.g. `ollama`): merges over the default runtime
    /// choice — `None` means "use the default".
    pub backend_override: Option<String>,
    /// Preload the weights at startup when true.
    pub preload: bool,
}

/// The parsed gallery index.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GalleryIndex {
    pub version: u32,
    pub models: Vec<GalleryEntry>,
}

// ---------------------------------------------------------------------------
// Id parsing
// ---------------------------------------------------------------------------

/// Split a `gallery@model` id into `(gallery, model)`.
///
/// Returns `None` when there is no `@`, either side is empty, or the model
/// side contains a second `@`.
pub fn parse_id(id: &str) -> Option<(String, String)> {
    let (gallery, model) = id.split_once('@')?;
    if gallery.is_empty() || model.is_empty() || model.contains('@') {
        return None;
    }
    Some((gallery.to_string(), model.to_string()))
}

// ---------------------------------------------------------------------------
// Minimal YAML subset parser (no new deps)
// ---------------------------------------------------------------------------

/// Parse the minimal gallery YAML subset:
///
/// ```yaml
/// version: 1
/// models:
///   - id: netlab@llama-3-8b
///     backend_override: ollama
///     preload: true
///     files:
///       - path: llama-3-8b.gguf
///         sha256: <64 hex chars>
/// ```
///
/// Recognized keys: `version:`, `models:`, `id:`, `backend_override:`,
/// `preload:` (`true`/`false`), `files:`, `path:`, `sha256:`. Unknown keys
/// are ignored; values may be single- or double-quoted. Missing `version:`
/// defaults to 1.
pub fn load_index_yaml(text: &str) -> Result<GalleryIndex, String> {
    let mut index = GalleryIndex {
        version: 1,
        models: Vec::new(),
    };
    let mut current: Option<GalleryEntry> = None;
    let mut pending: Option<GalleryFile> = None;

    // Push the pending file into the current entry. Errors on an incomplete
    // file (path without sha256) — a half-pinned file must never silently
    // pass as verified later.
    fn flush_file(
        current: &mut Option<GalleryEntry>,
        pending: &mut Option<GalleryFile>,
        n: usize,
    ) -> Result<(), String> {
        if let Some(f) = pending.take() {
            if f.path.is_empty() || f.sha256.is_empty() {
                return Err(format!(
                    "line {n}: incomplete gallery file (need path: + sha256:)"
                ));
            }
            match current.as_mut() {
                Some(e) => e.files.push(f),
                None => {
                    return Err(format!("line {n}: file outside any gallery entry"));
                }
            }
        }
        Ok(())
    }

    for (lineno, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let n = lineno + 1;

        if let Some(v) = strip_key(line, "version:") {
            index.version = unquote(v)
                .parse::<u32>()
                .map_err(|_| format!("line {n}: bad version `{v}`"))?;
            continue;
        }
        if is_key(line, "models:") || is_key(line, "files:") {
            continue;
        }
        // A new entry: `- id: <...>` (or a bare `-` followed by `id:`).
        if let Some(v) = strip_key(line, "- id:") {
            flush_file(&mut current, &mut pending, n)?;
            if let Some(e) = current.take() {
                if e.id.is_empty() {
                    return Err(format!("line {n}: gallery entry with empty id"));
                }
                index.models.push(e);
            }
            current = Some(GalleryEntry {
                id: unquote(v).to_string(),
                ..Default::default()
            });
            continue;
        }
        if line == "-" {
            flush_file(&mut current, &mut pending, n)?;
            if let Some(e) = current.take() {
                if e.id.is_empty() {
                    return Err(format!("line {n}: gallery entry with empty id"));
                }
                index.models.push(e);
            }
            current = Some(GalleryEntry::default());
            continue;
        }
        // A new file: `- path: <...>` (or `path:` continuing a pending file).
        if let Some(v) = strip_key(line, "- path:") {
            flush_file(&mut current, &mut pending, n)?;
            if current.is_none() {
                return Err(format!("line {n}: file outside any gallery entry"));
            }
            pending = Some(GalleryFile {
                path: unquote(v).to_string(),
                sha256: String::new(),
            });
            continue;
        }
        if let Some(v) = strip_key(line, "id:") {
            let e = current
                .as_mut()
                .ok_or_else(|| format!("line {n}: `id:` outside any gallery entry"))?;
            e.id = unquote(v).to_string();
            continue;
        }
        if let Some(v) = strip_key(line, "backend_override:") {
            let e = current.as_mut().ok_or_else(|| {
                format!("line {n}: `backend_override:` outside any gallery entry")
            })?;
            let val = unquote(v);
            e.backend_override = if val.is_empty() {
                None
            } else {
                Some(val.to_string())
            };
            continue;
        }
        if let Some(v) = strip_key(line, "preload:") {
            let e = current
                .as_mut()
                .ok_or_else(|| format!("line {n}: `preload:` outside any gallery entry"))?;
            e.preload = match unquote(v).to_ascii_lowercase().as_str() {
                "true" => true,
                "false" => false,
                other => return Err(format!("line {n}: bad preload `{other}` (want true/false)")),
            };
            continue;
        }
        if let Some(v) = strip_key(line, "path:") {
            if current.is_none() {
                return Err(format!("line {n}: file outside any gallery entry"));
            }
            match pending.as_mut() {
                Some(f) if f.path.is_empty() => f.path = unquote(v).to_string(),
                _ => {
                    flush_file(&mut current, &mut pending, n)?;
                    pending = Some(GalleryFile {
                        path: unquote(v).to_string(),
                        sha256: String::new(),
                    });
                }
            }
            continue;
        }
        if let Some(v) = strip_key(line, "sha256:") {
            let f = pending
                .as_mut()
                .ok_or_else(|| format!("line {n}: `sha256:` without a file `path:`"))?;
            f.sha256 = unquote(v).to_ascii_lowercase();
            continue;
        }
        // Unknown key — ignored for forward compatibility.
    }

    flush_file(&mut current, &mut pending, text.lines().count())?;
    if let Some(e) = current.take() {
        if e.id.is_empty() {
            return Err("gallery entry with empty id".into());
        }
        index.models.push(e);
    }
    Ok(index)
}

fn is_key(line: &str, key: &str) -> bool {
    line == key || line == key.trim_end_matches(':')
}

fn strip_key<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    // Accept both `key: value` and `- key: value` spellings (the dash forms
    // are matched by the caller first, so this is the plain form).
    let rest = line.strip_prefix(key)?;
    Some(rest.trim())
}

fn unquote(v: &str) -> &str {
    let v = v.trim();
    if v.len() >= 2 {
        let b = v.as_bytes();
        if (b[0] == b'"' && b[v.len() - 1] == b'"') || (b[0] == b'\'' && b[v.len() - 1] == b'\'') {
            return &v[1..v.len() - 1];
        }
    }
    v
}

// ---------------------------------------------------------------------------
// Verification (dependency-free streaming SHA-256)
// ---------------------------------------------------------------------------

/// Verify every pinned file of `entry` against `dir`.
///
/// Each `path` is joined onto `dir`; absolute paths and `..` escapes are
/// refused fail-closed. Returns `Ok(())` only when **every** file's sha256
/// matches (comparison is ASCII case-insensitive).
pub fn verify_files(entry: &GalleryEntry, dir: &Path) -> Result<(), String> {
    for f in &entry.files {
        let rel = Path::new(&f.path);
        if rel.is_absolute() || rel.components().any(|c| matches!(c, Component::ParentDir)) {
            return Err(format!("refusing path outside gallery dir: {}", f.path));
        }
        let full = dir.join(rel);
        let digest = sha256_file_hex(&full).map_err(|e| format!("read {}: {e}", f.path))?;
        if !digest.eq_ignore_ascii_case(&f.sha256) {
            return Err(format!(
                "sha256 mismatch for {}: expected {}, got {digest}",
                f.path, f.sha256
            ));
        }
    }
    Ok(())
}

fn sha256_file_hex(path: &Path) -> Result<String, String> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 128 * 1024];
    loop {
        match file.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => hasher.update(&buf[..n]),
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(hasher.finish_hex())
}

/// Minimal streaming SHA-256 (FIPS 180-4), so this crate needs no hash dep.
struct Sha256 {
    h: [u32; 8],
    buf: [u8; 64],
    buf_len: usize,
    total_len: u64,
}

impl Sha256 {
    fn new() -> Self {
        Self {
            h: [
                0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
                0x5be0cd19,
            ],
            buf: [0u8; 64],
            buf_len: 0,
            total_len: 0,
        }
    }

    fn update(&mut self, mut data: &[u8]) {
        self.total_len += data.len() as u64;
        if self.buf_len > 0 {
            let take = (64 - self.buf_len).min(data.len());
            self.buf[self.buf_len..self.buf_len + take].copy_from_slice(&data[..take]);
            self.buf_len += take;
            data = &data[take..];
            if self.buf_len == 64 {
                let block = self.buf;
                Self::compress(&mut self.h, &block);
                self.buf_len = 0;
            }
        }
        while data.len() >= 64 {
            let mut block = [0u8; 64];
            block.copy_from_slice(&data[..64]);
            Self::compress(&mut self.h, &block);
            data = &data[64..];
        }
        if !data.is_empty() {
            self.buf[..data.len()].copy_from_slice(data);
            self.buf_len = data.len();
        }
    }

    fn finish_hex(mut self) -> String {
        let bit_len = self.total_len.wrapping_mul(8);
        self.update(&[0x80]);
        while self.buf_len != 56 {
            self.update(&[0x00]);
        }
        self.update(&bit_len.to_be_bytes());
        debug_assert_eq!(self.buf_len, 0);
        let mut out = String::with_capacity(64);
        for w in self.h {
            out.push_str(&format!("{w:08x}"));
        }
        out
    }

    fn compress(h: &mut [u32; 8], block: &[u8; 64]) {
        const K: [u32; 64] = [
            0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
            0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
            0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
            0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
            0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
            0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
            0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
            0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
            0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
            0xc67178f2,
        ];
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                block[4 * i],
                block[4 * i + 1],
                block[4 * i + 2],
                block[4 * i + 3],
            ]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let (mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh) =
            (h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7]);
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("eaios-gallery-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn sha_of(bytes: &[u8]) -> String {
        let mut h = Sha256::new();
        h.update(bytes);
        h.finish_hex()
    }

    #[test]
    fn gallery_id_parses() {
        assert_eq!(
            parse_id("netlab@llama-3-8b"),
            Some(("netlab".to_string(), "llama-3-8b".to_string()))
        );
        assert_eq!(parse_id("no-at-sign"), None);
        assert_eq!(parse_id("@model"), None);
        assert_eq!(parse_id("gallery@"), None);
        assert_eq!(parse_id("a@b@c"), None);
    }

    #[test]
    fn gallery_verifies_each_file_sha() {
        // Known-answer check for the vendored SHA-256 (FIPS 180-4 §B.1).
        assert_eq!(
            sha_of(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let dir = tmp("verify");
        std::fs::write(dir.join("a.gguf"), b"abc").unwrap();
        std::fs::write(dir.join("b.gguf"), b"hello").unwrap();
        let entry = GalleryEntry {
            id: "netlab@two-files".into(),
            files: vec![
                GalleryFile {
                    path: "a.gguf".into(),
                    sha256: sha_of(b"abc"),
                },
                GalleryFile {
                    path: "b.gguf".into(),
                    sha256: sha_of(b"hello"),
                },
            ],
            backend_override: None,
            preload: false,
        };
        assert!(verify_files(&entry, &dir).is_ok());

        // Corrupting ANY single file must fail (each file is checked).
        std::fs::write(dir.join("b.gguf"), b"hello!").unwrap();
        let err = verify_files(&entry, &dir).unwrap_err();
        assert!(err.contains("b.gguf"), "error names the file: {err}");

        // Missing files and escapes fail closed.
        let missing = GalleryEntry {
            id: "netlab@missing".into(),
            files: vec![GalleryFile {
                path: "nope.gguf".into(),
                sha256: sha_of(b"x"),
            }],
            ..Default::default()
        };
        assert!(verify_files(&missing, &dir).is_err());
        let escape = GalleryEntry {
            id: "netlab@escape".into(),
            files: vec![GalleryFile {
                path: "../evil.gguf".into(),
                sha256: sha_of(b"x"),
            }],
            ..Default::default()
        };
        assert!(verify_files(&escape, &dir).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn override_merges() {
        let idx = load_index_yaml(
            "version: 1\n\
             models:\n\
             \x20 - id: netlab@llama-3-8b\n\
             \x20   backend_override: ollama\n\
             \x20   preload: false\n\
             \x20   files:\n\
             \x20     - path: llama.gguf\n\
             \x20       sha256: abc123\n\
             \x20 - id: netlab@phi-4\n\
             \x20   files:\n\
             \x20     - path: phi.gguf\n\
             \x20       sha256: def456\n",
        )
        .unwrap();
        assert_eq!(idx.version, 1);
        assert_eq!(idx.models.len(), 2);
        // The override merges over the default (no override = default runtime).
        assert_eq!(idx.models[0].backend_override.as_deref(), Some("ollama"));
        assert_eq!(idx.models[1].backend_override, None);
    }

    #[test]
    fn preload_flag_honored() {
        let idx = load_index_yaml(
            "version: 1\n\
             models:\n\
             \x20 - id: netlab@hot\n\
             \x20   preload: true\n\
             \x20   files:\n\
             \x20     - path: hot.gguf\n\
             \x20       sha256: abc123\n\
             \x20 - id: netlab@cold\n\
             \x20   preload: false\n\
             \x20   files:\n\
             \x20     - path: cold.gguf\n\
             \x20       sha256: def456\n",
        )
        .unwrap();
        assert!(idx.models[0].preload);
        assert!(!idx.models[1].preload);
        // Absent key defaults to false (lazy load).
        let idx2 = load_index_yaml(
            "models:\n\
             \x20 - id: netlab@lazy\n\
             \x20   files:\n\
             \x20     - path: lazy.gguf\n\
             \x20       sha256: abc123\n",
        )
        .unwrap();
        assert!(!idx2.models[0].preload);
    }
}
