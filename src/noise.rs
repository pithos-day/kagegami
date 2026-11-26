//! シェーダーの壁紙に渡す、組み込みの 3D ノイズ（#70）。
//!
//! - `@iChannelN noise3d-rgba`: 32³ の RGBA8。各バイトは 0〜255 の一様な乱数
//! - `@iChannelN noise3d-grey`: 32³ の R8。RGBA のバイト列の先頭 32³ バイトをそのまま使う
//!   （Shadertoy のグレーと RGBA の 3D ノイズのファイルと同じ関係）
//!
//! 乱数の種は固定なので、起動ごと・出力ごとに同じノイズになる。中身は Shadertoy のファイルとは違う。

use std::sync::OnceLock;

/// 1 辺のボクセル数。
pub const SIZE: u32 = 32;
const VOXELS: usize = (SIZE * SIZE * SIZE) as usize;

/// 乱数の種（"kagegami" の ASCII）。
const SEED: u64 = 0x6b61_6765_6761_6d69;

/// どちらのノイズか。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Noise {
    Grey,
    Rgba,
}

impl Noise {
    /// 1 ボクセルのバイト数。
    pub fn channels(self) -> usize {
        match self {
            Noise::Grey => 1,
            Noise::Rgba => 4,
        }
    }

    /// x が最も速く変わり、次に y、z の順に並べたボクセルのバイト列。
    pub fn data(self) -> &'static [u8] {
        let rgba = rgba();
        &rgba[..VOXELS * self.channels()]
    }
}

/// RGBA のノイズ。最初に使うときに 1 回だけ作る。
fn rgba() -> &'static [u8] {
    static DATA: OnceLock<Vec<u8>> = OnceLock::new();
    DATA.get_or_init(|| generate(SEED, VOXELS * 4))
}

/// splitmix64 で `len` バイトを作る（1 回で 8 バイト）。
fn generate(seed: u64, len: usize) -> Vec<u8> {
    let mut state = seed;
    let mut out = Vec::with_capacity(len.next_multiple_of(8));
    while out.len() < len {
        state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^= z >> 31;
        out.extend_from_slice(&z.to_le_bytes());
    }
    out.truncate(len);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_and_grey_is_the_head_of_rgba() {
        assert_eq!(Noise::Rgba.data().len(), 32 * 32 * 32 * 4);
        assert_eq!(Noise::Grey.data().len(), 32 * 32 * 32);
        assert_eq!(Noise::Grey.data(), &Noise::Rgba.data()[..32 * 32 * 32]);
    }

    #[test]
    fn same_noise_every_time() {
        assert_eq!(generate(SEED, VOXELS * 4), generate(SEED, VOXELS * 4));
        assert_eq!(generate(SEED, VOXELS * 4), Noise::Rgba.data());
        // 種が違えば違うノイズになる（種を使っていることの確認）。
        assert_ne!(generate(SEED + 1, VOXELS * 4), Noise::Rgba.data());
    }

    fn correlation(a: &[u8], b: &[u8]) -> f64 {
        let n = a.len() as f64;
        let mean = |v: &[u8]| v.iter().map(|&x| f64::from(x)).sum::<f64>() / n;
        let (ma, mb) = (mean(a), mean(b));
        let (mut cov, mut va, mut vb) = (0.0, 0.0, 0.0);
        for (&x, &y) in a.iter().zip(b) {
            let (dx, dy) = (f64::from(x) - ma, f64::from(y) - mb);
            cov += dx * dy;
            va += dx * dx;
            vb += dy * dy;
        }
        cov / (va * vb).sqrt()
    }

    /// 受入条件（#70）: Shadertoy の RGBA の 3D ノイズと同じように、一様で相関のない乱数になっている。
    /// Shadertoy のファイルは、各区間が平均の -1.6%〜+1.8%、隣接相関が x 0.0183・y -0.0002・z -0.0057、
    /// R と G の相関が 0.0061。
    #[test]
    fn statistics_are_close_to_shadertoy() {
        let rgba = Noise::Rgba.data();
        // 値の分布: 0〜255 を 32 刻みにした 8 区間それぞれの数が、平均から ±5% 以内。
        let mut bins = [0usize; 8];
        for &v in rgba {
            bins[usize::from(v / 32)] += 1;
        }
        let mean = rgba.len() as f64 / 8.0;
        for (i, &c) in bins.iter().enumerate() {
            let d = (c as f64 - mean) / mean;
            assert!(d.abs() <= 0.05, "bin {i}: {c} ({:+.2}%)", d * 100.0);
        }
        // R チャンネルの、隣り合うボクセルとの相関（端は折り返す）。
        let r: Vec<u8> = rgba.iter().step_by(4).copied().collect();
        let g: Vec<u8> = rgba.iter().skip(1).step_by(4).copied().collect();
        let s = SIZE as usize;
        let at = |x: usize, y: usize, z: usize| r[(x % s) + s * ((y % s) + s * (z % s))];
        for (axis, (dx, dy, dz)) in [("x", (1, 0, 0)), ("y", (0, 1, 0)), ("z", (0, 0, 1))] {
            let mut a = Vec::with_capacity(VOXELS);
            let mut b = Vec::with_capacity(VOXELS);
            for z in 0..s {
                for y in 0..s {
                    for x in 0..s {
                        a.push(at(x, y, z));
                        b.push(at(x + dx, y + dy, z + dz));
                    }
                }
            }
            let c = correlation(&a, &b);
            assert!(c.abs() <= 0.05, "neighbor correlation {axis}: {c}");
        }
        // チャンネル間（R と G）の相関。
        let c = correlation(&r, &g);
        assert!(c.abs() <= 0.05, "R-G correlation: {c}");
    }
}
