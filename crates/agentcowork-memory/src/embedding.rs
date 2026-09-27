//! Optional embedding path (C5 — doc 58/63): the on-device vector signal
//! (bge-micro / gte-small) that layers onto the vectorless BM25 default.
//!
//! The pure core here is the embedding math the coordinator uses once a model
//! is loaded: cosine / L2 / dot distance, int8 and vec0 (binary) quantization
//! for storage, and a nearest-neighbor index that backs both the semantic
//! cache (P1.3) and the optional vector retrieval signal (C5). The model
//! itself (ONNX runtime) is the caller's plug-in via [`Embedder`] — this crate
//! never loads weights.

/// The embedding model seam (ONNX bge-micro / gte-small / …). The caller
/// implements this; every function here is model-agnostic.
pub trait Embedder {
    fn embed(&self, text: &str) -> Vec<f32>;
}

/// Cosine similarity in [-1, 1] (empty vectors are dissimilar = -1.0).
pub fn cosine(a: &[f32], b: &[f32]) -> f64 {
    if a.is_empty() || b.is_empty() || a.len() != b.len() {
        return -1.0;
    }
    let mut dot = 0.0f64;
    let mut na = 0.0f64;
    let mut nb = 0.0f64;
    for (&x, &y) in a.iter().zip(b.iter()) {
        dot += x as f64 * y as f64;
        na += x as f64 * x as f64;
        nb += y as f64 * y as f64;
    }
    if na == 0.0 || nb == 0.0 {
        return -1.0;
    }
    (dot / (na.sqrt() * nb.sqrt())).clamp(-1.0, 1.0)
}

/// L2 distance (Euclidean). Larger = more dissimilar.
pub fn l2(a: &[f32], b: &[f32]) -> f64 {
    if a.len() != b.len() {
        return f64::INFINITY;
    }
    a.iter()
        .zip(b.iter())
        .map(|(x, y)| {
            let d = (x - y) as f64;
            d * d
        })
        .sum::<f64>()
        .sqrt()
}

/// Dot product.
pub fn dot(a: &[f32], b: &[f32]) -> f64 {
    a.iter()
        .zip(b.iter())
        .map(|(x, y)| *x as f64 * *y as f64)
        .sum()
}

/// Int8-quantized vector: `value ≈ scale * (q - zero_point)`.
#[derive(Debug, Clone, PartialEq)]
pub struct Int8Vector {
    pub values: Vec<i8>,
    pub scale: f32,
    pub zero_point: i8,
}

/// Symmetric int8 quantization (zero-point 0): `scale = max_abs / 127`,
/// `q = round(v / scale)` clamped to i8 range.
pub fn quantize_int8(v: &[f32]) -> Int8Vector {
    let max_abs = v.iter().fold(0.0f32, |m, &x| m.max(x.abs()));
    let scale = if max_abs == 0.0 { 1.0 } else { max_abs / 127.0 };
    let values = v
        .iter()
        .map(|&x| (x / scale).round().clamp(-128.0, 127.0) as i8)
        .collect();
    Int8Vector {
        values,
        scale,
        zero_point: 0,
    }
}

/// Binary (vec0) quantization: each component becomes one bit — `1` when the
/// value is `>= threshold`, else `0`. Bits are packed into `u64` words (LSB
/// first). Cosine/Hamming search over these is the vec0 fast path.
#[derive(Debug, Clone, PartialEq)]
pub struct BinaryVector {
    pub bits: Vec<u64>,
    /// The source dimension (reconstructable bit length).
    pub dim: usize,
    pub threshold: f32,
}

/// Binary-quantize a vector (vec0 pattern).
pub fn quantize_binary(v: &[f32], threshold: f32) -> BinaryVector {
    let words = v.len().div_ceil(64);
    let mut bits = vec![0u64; words];
    for (i, &x) in v.iter().enumerate() {
        if x >= threshold {
            bits[i / 64] |= 1u64 << (i % 64);
        }
    }
    BinaryVector {
        bits,
        dim: v.len(),
        threshold,
    }
}

/// Hamming distance between two binary vectors (same dim).
pub fn hamming(a: &BinaryVector, b: &BinaryVector) -> usize {
    a.bits
        .iter()
        .zip(b.bits.iter())
        .map(|(x, y)| (x ^ y).count_ones() as usize)
        .sum()
}

/// A brute-force nearest-neighbor index over stored embeddings (the vector
/// signal + semantic-cache backing store). Search returns `(id, cosine)`
/// sorted most-similar first.
#[derive(Debug, Clone, Default)]
pub struct EmbeddingIndex {
    entries: Vec<(String, Vec<f32>)>,
}

impl EmbeddingIndex {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Insert (or replace) an embedding by id.
    pub fn insert(&mut self, id: &str, vector: Vec<f32>) {
        if let Some(entry) = self.entries.iter_mut().find(|(i, _)| i == id) {
            entry.1 = vector;
            return;
        }
        self.entries.push((id.to_string(), vector));
    }

    /// Top-k most similar ids by cosine, best first. Empty for an empty query
    /// or index.
    pub fn search(&self, query: &[f32], k: usize) -> Vec<(String, f64)> {
        if query.is_empty() || self.entries.is_empty() || k == 0 {
            return Vec::new();
        }
        let mut scored: Vec<(String, f64)> = self
            .entries
            .iter()
            .map(|(id, v)| (id.clone(), cosine(query, v)))
            .collect();
        scored.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(Ordering::Equal)
                .then_with(|| a.0.cmp(&b.0))
        });
        scored.truncate(k);
        scored
    }

    /// The stored embedding for `id`, if any.
    pub fn get(&self, id: &str) -> Option<&[f32]> {
        self.entries
            .iter()
            .find(|(i, _)| i == id)
            .map(|(_, v)| v.as_slice())
    }
}

use std::cmp::Ordering;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cosine_identical_is_one_orthogonal_is_zero() {
        let a = [1.0f32, 0.0];
        let b = [1.0, 0.0];
        let c = [0.0, 1.0];
        assert!((cosine(&a, &b) - 1.0).abs() < 1e-6);
        assert!((cosine(&a, &c) - 0.0).abs() < 1e-6);
        assert!(cosine(&a, &[]).is_sign_negative() || cosine(&a, &[]) == -1.0);
    }

    #[test]
    fn l2_and_dot_agree_on_identical() {
        let a = [3.0f32, 4.0];
        assert_eq!(l2(&a, &a), 0.0);
        assert!((dot(&a, &a) - 25.0).abs() < 1e-6);
    }

    #[test]
    fn int8_quantization_round_trips_sign() {
        let v = [1.0f32, -1.0, 0.5, -0.5];
        let q = quantize_int8(&v);
        // Scale = 1/127 → q ≈ round(v*127).
        assert_eq!(q.values[0], 127);
        assert_eq!(q.values[1], -127);
        assert!(q.values[2] > 0);
        assert!(q.values[3] < 0);
    }

    #[test]
    fn int8_zero_vector_uses_unit_scale() {
        let q = quantize_int8(&[0.0, 0.0]);
        assert_eq!(q.scale, 1.0);
        assert_eq!(q.values, vec![0, 0]);
    }

    #[test]
    fn binary_quantization_packs_bits() {
        let v = [1.0f32, 0.0, 1.0, 0.0];
        let b = quantize_binary(&v, 0.5);
        assert_eq!(b.dim, 4);
        assert_eq!(b.bits[0], 0b0101); // LSB first: bits 0 and 2 set
        let same = quantize_binary(&v, 0.5);
        assert_eq!(hamming(&b, &same), 0);
        let flipped = quantize_binary(&[0.0, 1.0, 0.0, 1.0], 0.5);
        assert_eq!(hamming(&b, &flipped), 4);
    }

    #[test]
    fn index_search_returns_nearest_by_cosine() {
        let mut idx = EmbeddingIndex::new();
        idx.insert("cat", vec![1.0, 0.0, 0.0]);
        idx.insert("dog", vec![0.9, 0.1, 0.0]);
        idx.insert("car", vec![0.0, 1.0, 0.0]);
        let hits = idx.search(&[1.0, 0.0, 0.0], 2);
        assert_eq!(hits[0].0, "cat");
        assert!(hits[0].1 > hits[1].1);
        assert_eq!(idx.len(), 3);
        assert!(idx.get("cat").is_some());
        assert!(idx.get("zzz").is_none());
    }

    #[test]
    fn index_empty_queries() {
        let idx = EmbeddingIndex::new();
        assert!(idx.search(&[1.0], 5).is_empty());
        let mut idx2 = EmbeddingIndex::new();
        idx2.insert("x", vec![1.0]);
        assert!(idx2.search(&[], 5).is_empty());
    }

    #[test]
    fn index_insert_replaces_existing() {
        let mut idx = EmbeddingIndex::new();
        idx.insert("a", vec![1.0, 0.0]);
        idx.insert("a", vec![0.0, 1.0]);
        assert_eq!(idx.len(), 1);
        assert_eq!(idx.get("a").unwrap(), &[0.0, 1.0]);
    }
}
