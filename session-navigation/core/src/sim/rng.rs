//! SplitMix64: tiny, seedable, bit-for-bit reproducible. No OS randomness.

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Rng {
    state: u64,
}

const BASE62: &[u8; 62] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
const HEX: &[u8; 16] = b"0123456789abcdef";

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `0..n` (0 when `n` is 0).
    pub fn below(&mut self, n: u64) -> u64 {
        if n == 0 {
            0
        } else {
            self.next_u64() % n
        }
    }

    /// True with probability `percent`/100.
    pub fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }

    fn hex(&mut self, len: usize) -> String {
        (0..len)
            .map(|_| {
                let i = usize::try_from(self.below(16)).unwrap_or(0);
                char::from(HEX.get(i).copied().unwrap_or(b'0'))
            })
            .collect()
    }

    /// A random version-4 UUID (Claude CLI ids).
    pub fn uuid_v4(&mut self) -> String {
        let variant = ['8', '9', 'a', 'b'];
        let v = variant
            .get(usize::try_from(self.below(4)).unwrap_or(0))
            .copied()
            .unwrap_or('8');
        format!(
            "{}-{}-4{}-{}{}-{}",
            self.hex(8),
            self.hex(4),
            self.hex(3),
            v,
            self.hex(3),
            self.hex(12)
        )
    }

    /// A version-7 UUID with `millis` in the time field (Codex thread ids).
    pub fn uuid_v7(&mut self, millis: u64) -> String {
        let time = format!("{:012x}", millis & 0xFFFF_FFFF_FFFF);
        let (high, low) = time.split_at(8);
        format!(
            "{}-{}-7{}-8{}-{}",
            high,
            low,
            self.hex(3),
            self.hex(3),
            self.hex(12)
        )
    }

    /// `ses_` + 12 lowercase hex + 14 base62 (OpenCode ids).
    pub fn opencode_id(&mut self) -> String {
        let tail: String = (0..14)
            .map(|_| {
                let i = usize::try_from(self.below(62)).unwrap_or(0);
                char::from(BASE62.get(i).copied().unwrap_or(b'0'))
            })
            .collect();
        format!("ses_{}{}", self.hex(12), tail)
    }
}
