//! Warp-style merkle embedding sync (C5-gated — doc 08 §8.8 Warp embedding
//! sync pattern). Keeps a merkle tree of `file → chunk hashes` so that on each
//! `sync()` pass only *changed* chunks are re-embedded — untouched files are
//! never re-hashed past the top-level content hash, and unchanged chunks keep
//! their stored vectors verbatim.
//!
//! The [`Embedder`] seam matches `agentcowork-memory::embedding::Embedder`
//! (`fn embed(&self, text: &str) -> Vec<f32>`), so the optional on-device
//! embedding path plugs in as the caller's own backend — same discipline as
//! the C5 gating elsewhere: the model load is never this crate's concern.

use std::collections::BTreeMap;

/// Embedding backend seam (shape-identical to `agentcowork-memory`'s).
pub trait Embedder {
    fn embed(&self, text: &str) -> Vec<f32>;
}

/// Chunking strategy for embedding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChunkMode {
    /// Split on line boundaries into fixed-size line windows.
    Lines(usize),
    /// Split on token-ish boundaries by byte budget (approximate: whitespace groups).
    Bytes(usize),
}

/// One embeddable chunk with its storage identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    /// Stable id = `sha256(path + ":" + chunk_index + ":" + content_hash)`
    /// (first 16 hex chars — enough for dedupe, not a security boundary).
    pub id: String,
    pub path: String,
    pub index: usize,
    pub content_hash: String,
    pub text: String,
}

/// A changed chunk the sync pass must re-embed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangedChunk {
    pub id: String,
    pub path: String,
    pub index: usize,
    pub text: String,
}

/// Merkle state for one file: content hash + per-chunk hashes.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FileState {
    /// Hash of the whole file (top-level merkle node).
    pub content_hash: String,
    /// Per-chunk content hashes (index-aligned with the chunker output).
    pub chunk_hashes: Vec<String>,
}

/// Persistent merkle index: path → file state. Serializes to JSON for
/// `~/.agentcowork/codeintel/embedding-state.json` style persistence.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WarpIndex {
    pub files: BTreeMap<String, FileState>,
}

fn sha256_hex(input: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(input.as_bytes());
    format!("{:x}", h.finalize())[..16].to_string()
}

fn hash_of_bytes(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(bytes);
    format!("{:x}", h.finalize())[..16].to_string()
}

/// Split text into chunks per the mode.
pub fn chunk_text(text: &str, mode: ChunkMode) -> Vec<String> {
    match mode {
        ChunkMode::Lines(n) => {
            let n = n.max(1);
            text.lines()
                .collect::<Vec<_>>()
                .chunks(n)
                .map(|lines| lines.join("\n"))
                .filter(|c| !c.trim().is_empty())
                .collect()
        }
        ChunkMode::Bytes(budget) => {
            let budget = budget.max(64);
            let mut chunks = Vec::new();
            let mut current = String::new();
            for word in text.split_whitespace() {
                if current.len() + word.len() + 1 > budget && !current.is_empty() {
                    chunks.push(std::mem::take(&mut current));
                }
                if !current.is_empty() {
                    current.push(' ');
                }
                current.push_str(word);
            }
            if !current.trim().is_empty() {
                chunks.push(current);
            }
            chunks
        }
    }
}

/// Produce the full chunk set for one file (independent of any index).
pub fn chunks_for(path: &str, text: &str, mode: ChunkMode) -> Vec<Chunk> {
    chunk_text(text, mode)
        .into_iter()
        .enumerate()
        .map(|(i, chunk_text)| {
            let chunk_hash = hash_of_bytes(chunk_text.as_bytes());
            let id = sha256_hex(&format!("{path}:{i}:{chunk_hash}"));
            Chunk {
                id,
                path: path.to_string(),
                index: i,
                content_hash: chunk_hash,
                text: chunk_text,
            }
        })
        .collect()
}

/// The [`WarpIndex`] drives: only chunks whose file content hash changed
/// (or whose chunk hash differs from the stored state) are returned.
pub fn sync_changed(
    index: &WarpIndex,
    path: &str,
    text: &str,
    mode: ChunkMode,
) -> Vec<ChangedChunk> {
    let content_hash = hash_of_bytes(text.as_bytes());
    let stored = index.files.get(path);

    // Fast path: file byte-identical → nothing changed (top-level merkle node).
    if stored
        .as_ref()
        .is_some_and(|s| s.content_hash == content_hash)
    {
        return Vec::new();
    }

    // File changed: per-chunk compare against stored chunk hashes.
    let chunks = chunks_for(path, text, mode);
    let stored_hashes = stored.map(|s| s.chunk_hashes.as_slice()).unwrap_or(&[]);
    chunks
        .into_iter()
        .filter(|c| match stored_hashes.get(c.index) {
            Some(h) => *h != c.content_hash,
            None => true,
        })
        .map(|c| ChangedChunk {
            id: c.id,
            path: c.path,
            index: c.index,
            text: c.text,
        })
        .collect()
}

/// Apply a sync pass: embeds changed chunks through the injected [`Embedder`]
/// and records the new merkle state. Returns the fresh vectors keyed by chunk
/// id (caller owns the vector store).
pub fn embed_sync(
    index: &mut WarpIndex,
    embedder: &dyn Embedder,
    files: &[(String, String)],
    mode: ChunkMode,
) -> Vec<(String, Vec<f32>)> {
    let mut fresh = Vec::new();
    for (path, text) in files {
        let changed = sync_changed(index, path, text, mode);
        let content_hash = hash_of_bytes(text.as_bytes());
        let chunks = chunks_for(path, text, mode);
        let mut chunk_hashes = Vec::with_capacity(chunks.len());
        for c in &chunks {
            chunk_hashes.push(c.content_hash.clone());
            if changed.iter().any(|cc| cc.id == c.id) {
                fresh.push((c.id.clone(), embedder.embed(&c.text)));
            }
        }
        index.files.insert(
            path.clone(),
            FileState {
                content_hash,
                chunk_hashes,
            },
        );
    }
    fresh
}

#[cfg(test)]
mod tests {
    use super::*;

    struct StubEmbedder;
    impl Embedder for StubEmbedder {
        fn embed(&self, text: &str) -> Vec<f32> {
            // Deterministic fake: 4 dims, seeded by length + bytes.
            let n = text.len() as f32;
            vec![
                n,
                n / 2.0,
                1.0,
                text.bytes().map(f32::from).sum::<f32>() % 7.0,
            ]
        }
    }

    #[test]
    fn unchanged_file_is_merkle_skipped() {
        let mut idx = WarpIndex::default();
        let files = vec![("src/main.rs".to_string(), "fn main() {}\n".to_string())];
        let first = embed_sync(&mut idx, &StubEmbedder, &files, ChunkMode::Lines(50));
        assert_eq!(first.len(), 1, "first pass embeds the one chunk");
        // Second pass: byte-identical → top-level hash matches → zero re-embed.
        let second = embed_sync(&mut idx, &StubEmbedder, &files, ChunkMode::Lines(50));
        assert!(second.is_empty(), "merkle fast path skips unchanged files");
    }

    #[test]
    fn only_changed_chunks_reembed() {
        let mut idx = WarpIndex::default();
        let v1 = "line one\nline two\nline three\nline four\n".to_string();
        let v2 = "line one\nline two\nCHANGED\nline four\n".to_string();
        embed_sync(
            &mut idx,
            &StubEmbedder,
            &[("f.rs".to_string(), v1)],
            ChunkMode::Lines(2),
        );
        // v2 differs only in chunk index 1 (lines 3-4) of two line-pairs.
        let changed = sync_changed(&idx, "f.rs", &v2, ChunkMode::Lines(2));
        assert_eq!(changed.len(), 1, "only the touched chunk is dirty");
        assert_eq!(changed[0].index, 1);
    }

    #[test]
    fn chunking_modes_and_ids_are_stable() {
        let text = "alpha beta gamma\ndelta epsilon zeta\neta theta iota\nkappa lambda mu\n";
        let by_lines = chunk_text(text, ChunkMode::Lines(2));
        assert_eq!(by_lines.len(), 2);
        let chunks = chunks_for("a.txt", text, ChunkMode::Lines(2));
        assert_eq!(chunks.len(), 2);
        // Same input → same ids (dedupe identity).
        let again = chunks_for("a.txt", text, ChunkMode::Lines(2));
        assert_eq!(chunks[0].id, again[0].id);
        assert_ne!(chunks[0].id, chunks[1].id);
        // Bytes mode never returns an empty chunk.
        let by_bytes = chunk_text("x y z w v u", ChunkMode::Bytes(4));
        assert!(!by_bytes.is_empty());
        assert!(by_bytes.iter().all(|c| !c.trim().is_empty()));
    }

    #[test]
    fn new_file_embeds_all_and_records_state() {
        let mut idx = WarpIndex::default();
        let text = "fn a() {}\nfn b() {}\n".to_string();
        let fresh = embed_sync(
            &mut idx,
            &StubEmbedder,
            &[("lib.rs".to_string(), text.clone())],
            ChunkMode::Lines(10),
        );
        assert_eq!(fresh.len(), 1);
        let state = idx.files.get("lib.rs").expect("state recorded");
        assert_eq!(state.chunk_hashes.len(), 1);
    }
}
