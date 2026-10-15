//! Índice vectorial en memoria: matriz plana de `f32` (norma 1 => coseno = producto escalar) y
//! búsqueda exacta por fuerza bruta con prefiltro por metadatos.
//!
//! Decisión (ver docs/notes/rag.md): con ≤ ~100 k vectores la búsqueda exacta cuesta decenas de
//! ms y no hay aproximación que ajustar ni extensión C que mantener.

use std::{
    cmp::{Ordering, Reverse},
    collections::{BinaryHeap, HashMap, HashSet},
};

use crate::{embed::dot, model::DocKind};

/// Metadatos de documento que necesita el filtrado y el reranking (sin texto).
#[derive(Debug, Clone, PartialEq)]
pub struct DocMeta {
    pub id: i64,
    pub kind: DocKind,
    pub source: String,
    /// Emisor ya normalizado con `fold_key`.
    pub issuer_key: Option<String>,
    pub published_ms: i64,
    pub assets: Vec<String>,
}

#[derive(Debug, Clone, Copy)]
struct Row {
    chunk_id: i64,
    slot: u32,
    text_hash: u128,
}

#[derive(Debug, Clone, Default)]
pub struct Filter {
    pub assets: Vec<String>,
    pub include_general: bool,
    pub kinds: Vec<DocKind>,
    /// Ya normalizado con `fold_key`.
    pub issuer_key: Option<String>,
    pub since_ms: Option<i64>,
    pub until_ms: Option<i64>,
}

impl Filter {
    fn accepts(&self, d: &DocMeta) -> bool {
        if !self.kinds.is_empty() && !self.kinds.contains(&d.kind) {
            return false;
        }
        if self.since_ms.is_some_and(|s| d.published_ms < s) || self.until_ms.is_some_and(|u| d.published_ms > u) {
            return false;
        }
        if let Some(i) = &self.issuer_key
            && d.issuer_key.as_ref() != Some(i)
        {
            return false;
        }
        if !self.assets.is_empty() {
            let hit = d.assets.iter().any(|a| self.assets.contains(a));
            let general = d.assets.is_empty() && self.include_general;
            if !hit && !general {
                return false;
            }
        }
        true
    }
}

#[derive(Debug, Clone)]
pub struct Candidate {
    pub chunk_id: i64,
    pub doc_id: i64,
    pub slot: u32,
    pub similarity: f32,
    pub text_hash: u128,
}

#[derive(PartialEq)]
struct Scored(f32, usize);
impl Eq for Scored {}
impl PartialOrd for Scored {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for Scored {
    fn cmp(&self, o: &Self) -> Ordering {
        self.0.total_cmp(&o.0).then(o.1.cmp(&self.1))
    }
}

#[derive(Debug, Default)]
pub struct VectorIndex {
    dim: usize,
    vecs: Vec<f32>,
    rows: Vec<Row>,
    docs: Vec<Option<DocMeta>>,
    slot_of: HashMap<i64, u32>,
    free: Vec<u32>,
}

impl VectorIndex {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub fn dim(&self) -> Option<usize> {
        (self.dim > 0).then_some(self.dim)
    }

    pub fn doc_count(&self) -> usize {
        self.slot_of.len()
    }

    /// Memoria aproximada (vectores + filas + metadatos), en bytes.
    pub fn memory_bytes(&self) -> usize {
        self.vecs.capacity() * 4 + self.rows.capacity() * std::mem::size_of::<Row>() + self.docs.capacity() * std::mem::size_of::<Option<DocMeta>>()
    }

    pub fn doc(&self, slot: u32) -> Option<&DocMeta> {
        self.docs.get(slot as usize).and_then(Option::as_ref)
    }

    pub fn doc_by_id(&self, id: i64) -> Option<&DocMeta> {
        self.slot_of.get(&id).and_then(|s| self.doc(*s))
    }

    /// Alta o actualización de los metadatos de un documento.
    pub fn upsert_doc(&mut self, meta: DocMeta) {
        match self.slot_of.get(&meta.id) {
            Some(&s) => self.docs[s as usize] = Some(meta),
            None => {
                let id = meta.id;
                let slot = match self.free.pop() {
                    Some(s) => {
                        self.docs[s as usize] = Some(meta);
                        s
                    }
                    None => {
                        self.docs.push(Some(meta));
                        (self.docs.len() - 1) as u32
                    }
                };
                self.slot_of.insert(id, slot);
            }
        }
    }

    /// Añade un vector (ya normalizado). Error si la dimensión no coincide o el documento no existe.
    pub fn add(&mut self, chunk_id: i64, doc_id: i64, text_hash: u128, vec: &[f32]) -> Result<(), String> {
        let slot = *self.slot_of.get(&doc_id).ok_or_else(|| format!("documento {doc_id} desconocido"))?;
        if self.dim == 0 {
            self.dim = vec.len();
        }
        if vec.len() != self.dim || vec.is_empty() {
            return Err(format!("dimensión {} distinta de la del índice ({})", vec.len(), self.dim));
        }
        self.vecs.extend_from_slice(vec);
        self.rows.push(Row { chunk_id, slot, text_hash });
        Ok(())
    }

    /// Quita documentos y todos sus vectores en una sola pasada.
    pub fn remove_docs(&mut self, ids: &HashSet<i64>) -> usize {
        let slots: HashSet<u32> = ids.iter().filter_map(|i| self.slot_of.get(i).copied()).collect();
        if slots.is_empty() {
            return 0;
        }
        let dim = self.dim;
        let mut w = 0usize;
        for r in 0..self.rows.len() {
            if slots.contains(&self.rows[r].slot) {
                continue;
            }
            if w != r {
                self.rows[w] = self.rows[r];
                self.vecs.copy_within(r * dim..(r + 1) * dim, w * dim);
            }
            w += 1;
        }
        let removed = self.rows.len() - w;
        self.rows.truncate(w);
        self.vecs.truncate(w * dim);
        for id in ids {
            if let Some(s) = self.slot_of.remove(id) {
                self.docs[s as usize] = None;
                self.free.push(s);
            }
        }
        removed
    }

    /// Los `n` vectores más parecidos que pasan el filtro, de mayor a menor similitud. También
    /// devuelve cuántos se compararon.
    pub fn search(&self, query: &[f32], filter: &Filter, n: usize, min_similarity: Option<f32>) -> (Vec<Candidate>, usize) {
        if self.rows.is_empty() || query.len() != self.dim || n == 0 {
            return (Vec::new(), 0);
        }
        let mut heap: BinaryHeap<Reverse<Scored>> = BinaryHeap::with_capacity(n + 1);
        let mut scanned = 0usize;
        // Resultado del filtro por slot (una vez por documento, no por fragmento).
        let ok: Vec<bool> = self.docs.iter().map(|d| d.as_ref().is_some_and(|d| filter.accepts(d))).collect();
        for (i, row) in self.rows.iter().enumerate() {
            if !ok[row.slot as usize] {
                continue;
            }
            scanned += 1;
            let s = dot(query, &self.vecs[i * self.dim..(i + 1) * self.dim]);
            if min_similarity.is_some_and(|m| s < m) {
                continue;
            }
            if heap.len() < n {
                heap.push(Reverse(Scored(s, i)));
            } else if heap.peek().is_some_and(|Reverse(w)| Scored(s, i) > *w) {
                heap.pop();
                heap.push(Reverse(Scored(s, i)));
            }
        }
        let mut best: Vec<Scored> = heap.into_iter().map(|Reverse(s)| s).collect();
        best.sort_by(|a, b| b.cmp(a));
        let out = best
            .into_iter()
            .map(|Scored(sim, i)| {
                let r = self.rows[i];
                let doc_id = self.doc(r.slot).map_or(0, |d| d.id);
                Candidate { chunk_id: r.chunk_id, doc_id, slot: r.slot, similarity: sim, text_hash: r.text_hash }
            })
            .collect();
        (out, scanned)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(id: i64, kind: DocKind, assets: &[&str], published_ms: i64) -> DocMeta {
        DocMeta { id, kind, source: "t".into(), issuer_key: None, published_ms, assets: assets.iter().map(|s| s.to_string()).collect() }
    }

    fn unit(seed: u64, dim: usize) -> Vec<f32> {
        let mut s = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        let mut v: Vec<f32> = (0..dim)
            .map(|_| {
                s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                ((s >> 33) as f32 / (1u64 << 31) as f32) - 0.5
            })
            .collect();
        let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        v.iter_mut().for_each(|x| *x /= n);
        v
    }

    fn build(n: usize) -> (VectorIndex, Vec<Vec<f32>>) {
        let mut ix = VectorIndex::new();
        let mut all = Vec::new();
        for d in 0..(n / 3) as i64 {
            ix.upsert_doc(meta(d, if d % 2 == 0 { DocKind::News } else { DocKind::Filing }, &[if d % 3 == 0 { "stock:AAPL" } else { "stock:MSFT" }], d * 1000));
        }
        for i in 0..n {
            let v = unit(i as u64 + 1, 16);
            ix.add(i as i64, (i / 3) as i64, i as u128, &v).unwrap();
            all.push(v);
        }
        (ix, all)
    }

    #[test]
    fn top_k_matches_naive_sort() {
        let (ix, all) = build(300);
        let q = unit(9999, 16);
        let (got, scanned) = ix.search(&q, &Filter::default(), 10, None);
        assert_eq!(scanned, 300);
        let mut naive: Vec<(f32, usize)> = all.iter().enumerate().map(|(i, v)| (dot(&q, v), i)).collect();
        naive.sort_by(|a, b| b.0.total_cmp(&a.0));
        let want: Vec<i64> = naive.iter().take(10).map(|x| x.1 as i64).collect();
        assert_eq!(got.iter().map(|c| c.chunk_id).collect::<Vec<_>>(), want);
    }

    #[test]
    fn filters_by_asset_kind_date_and_general() {
        let (mut ix, _) = build(90);
        let q = unit(5, 16);
        let f = Filter { assets: vec!["stock:AAPL".into()], ..Default::default() };
        let (hits, scanned) = ix.search(&q, &f, 1000, None);
        assert!(scanned > 0 && scanned < 90);
        assert!(hits.iter().all(|h| ix.doc(h.slot).unwrap().assets == ["stock:AAPL"]));
        let f = Filter { kinds: vec![DocKind::Filing], since_ms: Some(5_000), until_ms: Some(20_000), ..Default::default() };
        let (hits, _) = ix.search(&q, &f, 1000, None);
        assert!(!hits.is_empty());
        assert!(hits.iter().all(|h| { let d = ix.doc(h.slot).unwrap(); d.kind == DocKind::Filing && (5_000..=20_000).contains(&d.published_ms) }));
        ix.upsert_doc(meta(500, DocKind::News, &[], 0));
        ix.add(900, 500, 900, &unit(77, 16)).unwrap();
        let strict = Filter { assets: vec!["stock:AAPL".into()], ..Default::default() };
        let loose = Filter { include_general: true, ..strict.clone() };
        assert!(!ix.search(&q, &strict, 1000, None).0.iter().any(|c| c.chunk_id == 900));
        assert!(ix.search(&q, &loose, 1000, None).0.iter().any(|c| c.chunk_id == 900));
    }

    #[test]
    fn removal_compacts_without_corrupting_neighbours() {
        let (mut ix, all) = build(60);
        let removed = ix.remove_docs(&HashSet::from([0, 5, 19]));
        assert_eq!(removed, 9);
        assert_eq!(ix.len(), 51);
        // Cada vector restante sigue siendo el suyo.
        for (i, v) in all.iter().enumerate() {
            let doc = (i / 3) as i64;
            let (hits, _) = ix.search(v, &Filter::default(), 1, None);
            if [0, 5, 19].contains(&doc) {
                assert_ne!(hits[0].chunk_id, i as i64);
            } else {
                assert_eq!((hits[0].chunk_id, hits[0].similarity > 0.999), (i as i64, true));
            }
        }
        // El hueco se reutiliza.
        ix.upsert_doc(meta(1000, DocKind::News, &[], 1));
        ix.add(1000, 1000, 1, &unit(3, 16)).unwrap();
        assert_eq!(ix.doc_count(), 18);
    }

    #[test]
    fn rejects_wrong_dimension_and_unknown_doc() {
        let mut ix = VectorIndex::new();
        ix.upsert_doc(meta(1, DocKind::News, &[], 0));
        ix.add(1, 1, 0, &[1.0, 0.0]).unwrap();
        assert!(ix.add(2, 1, 0, &[1.0, 0.0, 0.0]).is_err());
        assert!(ix.add(3, 99, 0, &[1.0, 0.0]).is_err());
        assert!(ix.search(&[1.0], &Filter::default(), 3, None).0.is_empty());
    }

    #[test]
    fn min_similarity_cuts() {
        let (ix, _) = build(30);
        let q = unit(1, 16);
        let (all, _) = ix.search(&q, &Filter::default(), 100, None);
        let (cut, _) = ix.search(&q, &Filter::default(), 100, Some(0.9));
        assert!(cut.len() < all.len() && cut.iter().all(|c| c.similarity >= 0.9));
    }
}
