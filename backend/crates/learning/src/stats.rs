//! Utilidades estadísticas sin dependencias: PRNG determinista, álgebra pequeña, correlación de
//! rangos, intervalo de Wilson y bootstrap por bloques (clusters).

use std::collections::BTreeMap;

/// splitmix64: pequeño, determinista y suficiente para remuestreo y datos sintéticos.
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0xD1B5_4A32_D192_ED03)
    }
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    /// Uniforme en [0, 1).
    pub fn uniform(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
    /// Normal estándar (Box–Muller).
    pub fn normal(&mut self) -> f64 {
        let u1 = self.uniform().max(1e-300);
        let u2 = self.uniform();
        (-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos()
    }
    pub fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n.max(1) as u64) as usize
    }
}

#[allow(clippy::needless_range_loop)]
/// Resuelve `A x = b` por eliminación gaussiana con pivote parcial. `None` si es singular.
pub fn solve(mut a: Vec<Vec<f64>>, mut b: Vec<f64>) -> Option<Vec<f64>> {
    let n = b.len();
    for col in 0..n {
        let piv = (col..n).max_by(|&i, &j| a[i][col].abs().total_cmp(&a[j][col].abs()))?;
        if a[piv][col].abs() < 1e-12 {
            return None;
        }
        a.swap(col, piv);
        b.swap(col, piv);
        for r in col + 1..n {
            let f = a[r][col] / a[col][col];
            for c in col..n {
                a[r][c] -= f * a[col][c];
            }
            b[r] -= f * b[col];
        }
    }
    let mut x = vec![0.0; n];
    for i in (0..n).rev() {
        let s: f64 = (i + 1..n).map(|j| a[i][j] * x[j]).sum();
        x[i] = (b[i] - s) / a[i][i];
    }
    Some(x)
}

/// Rangos medios (los empates reciben el promedio).
pub fn ranks(x: &[f64]) -> Vec<f64> {
    let mut idx: Vec<usize> = (0..x.len()).collect();
    idx.sort_by(|&a, &b| x[a].total_cmp(&x[b]));
    let mut r = vec![0.0; x.len()];
    let mut i = 0;
    while i < idx.len() {
        let mut j = i;
        while j + 1 < idx.len() && x[idx[j + 1]] == x[idx[i]] {
            j += 1;
        }
        let avg = (i + j) as f64 / 2.0 + 1.0;
        for &k in &idx[i..=j] {
            r[k] = avg;
        }
        i = j + 1;
    }
    r
}

pub fn pearson(x: &[f64], y: &[f64]) -> Option<f64> {
    let n = x.len();
    if n < 3 || y.len() != n {
        return None;
    }
    let (mx, my) = (
        x.iter().sum::<f64>() / n as f64,
        y.iter().sum::<f64>() / n as f64,
    );
    let (mut sxy, mut sxx, mut syy) = (0.0, 0.0, 0.0);
    for i in 0..n {
        let (dx, dy) = (x[i] - mx, y[i] - my);
        sxy += dx * dy;
        sxx += dx * dx;
        syy += dy * dy;
    }
    if sxx <= 1e-18 || syy <= 1e-18 {
        None
    } else {
        Some(sxy / (sxx * syy).sqrt())
    }
}

/// Correlación de rangos de Spearman.
pub fn spearman(x: &[f64], y: &[f64]) -> Option<f64> {
    pearson(&ranks(x), &ranks(y))
}

/// Intervalo de Wilson al 95 % para una proporción.
pub fn wilson(k: usize, n: usize) -> Option<(f64, f64)> {
    if n == 0 {
        return None;
    }
    let (z, n_f, p) = (1.96f64, n as f64, k as f64 / n as f64);
    let d = 1.0 + z * z / n_f;
    let c = (p + z * z / (2.0 * n_f)) / d;
    let h = z * (p * (1.0 - p) / n_f + z * z / (4.0 * n_f * n_f)).sqrt() / d;
    Some(((c - h).max(0.0), (c + h).min(1.0)))
}

pub fn median(v: &[f64]) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    let mut s = v.to_vec();
    s.sort_by(f64::total_cmp);
    let m = s.len() / 2;
    if s.len() % 2 == 1 {
        s[m]
    } else {
        (s[m - 1] + s[m]) / 2.0
    }
}

/// Cuantil por interpolación lineal sobre un vector ordenado.
pub fn quantile_sorted(s: &[f64], q: f64) -> f64 {
    if s.is_empty() {
        return f64::NAN;
    }
    let pos = q.clamp(0.0, 1.0) * (s.len() - 1) as f64;
    let (lo, hi) = (pos.floor() as usize, pos.ceil() as usize);
    s[lo] + (s[hi] - s[lo]) * (pos - lo as f64)
}

/// Bootstrap por bloques: remuestrea clusters enteros con reposición y evalúa `stat` sobre los
/// índices resultantes. Respeta la dependencia dentro de cada bloque (misma semana de mercado).
pub fn cluster_bootstrap(
    clusters: &[u64],
    reps: usize,
    seed: u64,
    mut stat: impl FnMut(&[usize]) -> Option<f64>,
) -> Vec<f64> {
    let mut groups: BTreeMap<u64, Vec<usize>> = BTreeMap::new();
    for (i, c) in clusters.iter().enumerate() {
        groups.entry(*c).or_default().push(i);
    }
    let groups: Vec<Vec<usize>> = groups.into_values().collect();
    let m = groups.len();
    let mut rng = Rng::new(seed);
    let mut out = Vec::with_capacity(reps);
    let mut idx = Vec::with_capacity(clusters.len());
    if m == 0 {
        return out;
    }
    for _ in 0..reps {
        idx.clear();
        for _ in 0..m {
            idx.extend_from_slice(&groups[rng.below(m)]);
        }
        if let Some(v) = stat(&idx) {
            out.push(v);
        }
    }
    out
}

pub fn n_clusters(clusters: &[u64]) -> usize {
    let mut c = clusters.to_vec();
    c.sort_unstable();
    c.dedup();
    c.len()
}

/// IC 95 % percentil de las réplicas.
pub fn percentile_ci(reps: &mut [f64]) -> Option<(f64, f64)> {
    if reps.len() < 50 {
        return None;
    }
    reps.sort_by(f64::total_cmp);
    Some((quantile_sorted(reps, 0.025), quantile_sorted(reps, 0.975)))
}

/// Mínimo de bloques independientes para ofrecer un intervalo (con menos, se omite).
pub const MIN_CLUSTERS_FOR_CI: usize = 5;

/// IC 95 % de la media de `values` con bootstrap por bloques.
pub fn mean_ci(values: &[f64], clusters: &[u64], reps: usize, seed: u64) -> Option<(f64, f64)> {
    if n_clusters(clusters) < MIN_CLUSTERS_FOR_CI {
        return None;
    }
    let mut r = cluster_bootstrap(clusters, reps, seed, |idx| {
        Some(idx.iter().map(|&i| values[i]).sum::<f64>() / idx.len() as f64)
    });
    percentile_ci(&mut r)
}
