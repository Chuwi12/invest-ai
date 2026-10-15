//! Contrato de embeddings y utilidades vectoriales, más una implementación falsa determinista.

use async_trait::async_trait;

use crate::error::EmbedError;

/// Cómo se usa el texto: los modelos asimétricos (nomic, gemma, qwen3) aplican prefijos distintos.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbedKind {
    Document,
    Query,
}

/// Proveedor de embeddings. Los vectores devueltos deben tener norma L2 = 1 (el llamador lo
/// valida y los normaliza si hace falta), todos con la misma dimensión.
#[async_trait]
pub trait Embedder: Send + Sync + 'static {
    /// Identifica el ESPACIO vectorial (modelo + estilo de prompt + dimensiones). Dos
    /// identificadores distintos nunca se mezclan en el índice.
    fn model_id(&self) -> String;
    async fn embed(&self, texts: &[String], kind: EmbedKind) -> Result<Vec<Vec<f32>>, EmbedError>;
}

/// Normaliza a norma 1. Error si hay NaN/inf o la norma es 0.
pub fn normalize_l2(v: &mut [f32]) -> Result<(), EmbedError> {
    if v.iter().any(|x| !x.is_finite()) {
        return Err(EmbedError::Bad("el vector contiene NaN o infinitos".into()));
    }
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm < 1e-12 {
        return Err(EmbedError::Bad("vector de norma cero".into()));
    }
    v.iter_mut().for_each(|x| *x /= norm);
    Ok(())
}

pub fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

pub fn to_blob(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}

pub fn from_blob(b: &[u8]) -> Option<Vec<f32>> {
    if !b.len().is_multiple_of(4) {
        return None;
    }
    Some(b.as_chunks::<4>().0.iter().map(|c| f32::from_le_bytes(*c)).collect())
}

fn fnv1a(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// Embedder falso y determinista (bolsa de palabras con «hashing trick»): textos que comparten
/// palabras tienen vectores parecidos. Para tests y el modo demo; NO es semántico.
#[derive(Debug, Clone)]
pub struct FakeEmbedder {
    pub dim: usize,
}

impl Default for FakeEmbedder {
    fn default() -> Self {
        Self { dim: 256 }
    }
}

impl FakeEmbedder {
    pub fn vector(&self, text: &str) -> Vec<f32> {
        let mut v = vec![0f32; self.dim.max(2)];
        for w in text.split(|c: char| !c.is_alphanumeric()).filter(|w| w.chars().count() >= 2) {
            let h = fnv1a(&w.to_lowercase());
            let idx = (h % v.len() as u64) as usize;
            v[idx] += if (h >> 40) & 1 == 0 { 1.0 } else { -1.0 };
        }
        if normalize_l2(&mut v).is_err() {
            v.iter_mut().for_each(|x| *x = 0.0);
            v[0] = 1.0;
        }
        v
    }
}

#[async_trait]
impl Embedder for FakeEmbedder {
    fn model_id(&self) -> String {
        format!("fake|hash|{}", self.dim)
    }

    async fn embed(&self, texts: &[String], _kind: EmbedKind) -> Result<Vec<Vec<f32>>, EmbedError> {
        Ok(texts.iter().map(|t| self.vector(t)).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn fake_is_deterministic_normalised_and_lexical() {
        let e = FakeEmbedder::default();
        let v = e.embed(&["Apple vende iPhone".into(), "Apple vende iPhone".into(), "Bitcoin ETF flujos".into()], EmbedKind::Document).await.unwrap();
        assert_eq!(v[0], v[1]);
        assert!((dot(&v[0], &v[0]) - 1.0).abs() < 1e-5);
        assert!(dot(&v[0], &v[2]) < 0.5);
        let q = e.vector("iphone apple ventas");
        assert!(dot(&q, &v[0]) > dot(&q, &v[2]));
    }

    #[test]
    fn blob_round_trip_and_validation() {
        let v = vec![0.5f32, -1.25, 3.0];
        assert_eq!(from_blob(&to_blob(&v)).unwrap(), v);
        assert!(from_blob(&[1, 2, 3]).is_none());
        assert!(normalize_l2(&mut [f32::NAN, 1.0]).is_err());
        assert!(normalize_l2(&mut [0.0, 0.0]).is_err());
        let mut x = [3.0, 4.0];
        normalize_l2(&mut x).unwrap();
        assert!((x[0] - 0.6).abs() < 1e-6);
    }
}
