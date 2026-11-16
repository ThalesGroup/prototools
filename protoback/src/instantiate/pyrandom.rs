// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! CPython's `random.Random` (spec 0432 S2): the Mersenne Twister
//! (MT19937) as `_randommodule.c` seeds and draws from it, and the methods
//! of `random.py` reproto's instances use, draw for draw.

const N: usize = 624;
const M: usize = 397;
const MATRIX_A: u32 = 0x9908_b0df;
const UPPER_MASK: u32 = 0x8000_0000;
const LOWER_MASK: u32 = 0x7fff_ffff;

pub struct Random {
    mt: [u32; N],
    index: usize,
}

impl Random {
    /// `random.Random(n)` for a non-negative integer, given as its
    /// little-endian bytes: `init_by_array` over its 32-bit words, least
    /// significant first, as many as its bits need (one for zero).
    pub fn from_int_le(bytes: &[u8]) -> Random {
        let mut words: Vec<u32> = bytes
            .chunks(4)
            .map(|c| {
                let mut w = [0u8; 4];
                w[..c.len()].copy_from_slice(c);
                u32::from_le_bytes(w)
            })
            .collect();
        while words.len() > 1 && words.last() == Some(&0) {
            words.pop();
        }
        if words.is_empty() {
            words.push(0);
        }
        let mut r = Random {
            mt: [0; N],
            index: N,
        };
        r.init_by_array(&words);
        r
    }

    fn init_genrand(&mut self, s: u32) {
        self.mt[0] = s;
        for i in 1..N {
            let prev = self.mt[i - 1];
            self.mt[i] = 1_812_433_253u32
                .wrapping_mul(prev ^ (prev >> 30))
                .wrapping_add(i as u32);
        }
        self.index = N;
    }

    fn init_by_array(&mut self, key: &[u32]) {
        self.init_genrand(19_650_218);
        let (mut i, mut j) = (1usize, 0usize);
        for _ in 0..N.max(key.len()) {
            let prev = self.mt[i - 1];
            self.mt[i] = (self.mt[i] ^ (prev ^ (prev >> 30)).wrapping_mul(1_664_525))
                .wrapping_add(key[j])
                .wrapping_add(j as u32);
            i += 1;
            j += 1;
            if i >= N {
                self.mt[0] = self.mt[N - 1];
                i = 1;
            }
            if j >= key.len() {
                j = 0;
            }
        }
        for _ in 0..N - 1 {
            let prev = self.mt[i - 1];
            self.mt[i] = (self.mt[i] ^ (prev ^ (prev >> 30)).wrapping_mul(1_566_083_941))
                .wrapping_sub(i as u32);
            i += 1;
            if i >= N {
                self.mt[0] = self.mt[N - 1];
                i = 1;
            }
        }
        self.mt[0] = 0x8000_0000;
    }

    /// `genrand_uint32`.
    fn next_u32(&mut self) -> u32 {
        if self.index >= N {
            for k in 0..N {
                let y = (self.mt[k] & UPPER_MASK) | (self.mt[(k + 1) % N] & LOWER_MASK);
                let mag = if y & 1 == 1 { MATRIX_A } else { 0 };
                self.mt[k] = self.mt[(k + M) % N] ^ (y >> 1) ^ mag;
            }
            self.index = 0;
        }
        let mut y = self.mt[self.index];
        self.index += 1;
        y ^= y >> 11;
        y ^= (y << 7) & 0x9d2c_5680;
        y ^= (y << 15) & 0xefc6_0000;
        y ^= y >> 18;
        y
    }

    /// `getrandbits(k)`, for `k` up to 64: 32-bit words, least significant
    /// first, the last one cut to the bits left.
    pub fn getrandbits(&mut self, k: u32) -> u64 {
        assert!(k <= 64, "getrandbits({k}): at most 64 bits");
        if k == 0 {
            return 0;
        }
        if k <= 32 {
            return u64::from(self.next_u32() >> (32 - k));
        }
        let low = u64::from(self.next_u32());
        let high = u64::from(self.next_u32() >> (64 - k));
        low | (high << 32)
    }

    /// `random()`: 53 bits from two draws.
    pub fn random(&mut self) -> f64 {
        let a = f64::from(self.next_u32() >> 5);
        let b = f64::from(self.next_u32() >> 6);
        (a * 67_108_864.0 + b) * (1.0 / 9_007_199_254_740_992.0)
    }

    /// `_randbelow(n)`: `getrandbits` of `n`'s bit length, until below `n`;
    /// 0 for 0.
    pub fn randbelow(&mut self, n: u64) -> u64 {
        if n == 0 {
            return 0;
        }
        let k = 64 - n.leading_zeros();
        loop {
            let r = self.getrandbits(k);
            if r < n {
                return r;
            }
        }
    }

    /// `randint(a, b)`, `a <= b`: `randrange(a, b + 1)`.
    pub fn randint(&mut self, a: i64, b: i64) -> i64 {
        assert!(a <= b, "randint({a}, {b}): empty range");
        let width = (b as i128 - a as i128 + 1) as u64;
        (a as i128 + self.randbelow(width) as i128) as i64
    }

    /// `choice(seq)` for a sequence of `len` items: the index chosen.
    pub fn choice(&mut self, len: usize) -> usize {
        assert!(len > 0, "Cannot choose from an empty sequence");
        self.randbelow(len as u64) as usize
    }

    /// `uniform(a, b)`.
    pub fn uniform(&mut self, a: f64, b: f64) -> f64 {
        a + (b - a) * self.random()
    }
}

#[cfg(test)]
mod tests {
    use sha2::{Digest, Sha256};

    use super::Random;

    mod cases {
        include!("pyrandom_cases.rs");
    }

    #[test]
    fn random_matches_cpython() {
        for &(seed, method, expected) in cases::SEQUENCES {
            let mut r = Random::from_int_le(cases::SEEDS[seed]);
            let mut h = Sha256::new();
            for _ in 0..cases::DRAWS {
                let bytes = match method {
                    "getrandbits1" => r.getrandbits(1).to_le_bytes(),
                    "getrandbits13" => r.getrandbits(13).to_le_bytes(),
                    "getrandbits32" => r.getrandbits(32).to_le_bytes(),
                    "getrandbits64" => r.getrandbits(64).to_le_bytes(),
                    "random" => r.random().to_bits().to_le_bytes(),
                    "randint_1_9999" => r.randint(1, 9999).to_le_bytes(),
                    "randint_0_255" => r.randint(0, 255).to_le_bytes(),
                    "randint_0_1" => r.randint(0, 1).to_le_bytes(),
                    "randint_1_1000" => r.randint(1, 1000).to_le_bytes(),
                    "uniform_1_1000" => r.uniform(1.0, 1000.0).to_bits().to_le_bytes(),
                    "choice_7" => (r.choice(7) as i64).to_le_bytes(),
                    other => panic!("no method {other}"),
                };
                h.update(bytes);
            }
            let got: String = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
            assert_eq!(got, expected, "seed {seed}, {method}");
        }
    }
}
