//! BitKnit 2 decompressor (Granny compression type 4), the only section codec used by BG3.
//!
//! BitKnit is an LZ77 + rANS codec by RAD Game Tools. Summary of the bitstream (see
//! `docs/formats/gr2.md` for the full description):
//!
//! * The stream is a sequence of little-endian 16-bit words, starting with the magic `0x75B1`.
//! * Output is produced in 64 KiB *quanta* (boundaries relative to the start of the output). A
//!   quantum either starts with a zero word (stored: raw bytes follow) or with an init block for
//!   two interleaved 32-bit rANS states (L = 2^16, 16-bit renormalisation, 15-bit probabilities).
//!   The coders alternate after every symbol / raw-bit extraction. At the end of each coded
//!   quantum both states must be back at 2^16.
//! * Three adaptive symbol alphabets: command words (300 symbols: 256 delta-literals, 44 copy
//!   lengths), copy-offset cache references (40: 8 cache hits, 32 offset low bits), and
//!   copy-offset bit lengths (21). Command and cache-ref models come in four instances selected by
//!   `output position % 4`. Models re-derive their CDF every 1024 symbols, averaging the old CDF
//!   with the freshly counted one.
//! * Literals are deltas against the byte at `pos - last_copy_offset`.
//! * Copy offsets are kept in an 8-entry LRU cache whose order is a packed nibble permutation.
//!
//! Format knowledge comes from Mackenzie Straight's libbg3 `docs/bitknit2.txt` and pybg3
//! (MIT), plus Fabian Giesen's public blog posts on BitKnit. This implementation is written from
//! that description. The game's own decoder is `granny::BitKnitDecompress` (4.1.1.7631656).

use std::fmt;

/// First word of every BitKnit 2 stream.
pub const MAGIC: u16 = 0x75B1;

const QUANTUM: usize = 0x1_0000;
const PROB_BITS: u32 = 15;
const PROB_TOTAL: u32 = 1 << PROB_BITS;
const LOOKUP_BITS: u32 = 10;
const LOOKUP_SHIFT: u32 = PROB_BITS - LOOKUP_BITS;
const ADAPT_INTERVAL: u32 = 1024;
const RANS_L: u32 = 1 << 16;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BitKnitError {
    BadMagic,
    SourceExhausted,
    BadCopyOffset { offset: u32, pos: usize },
    BadCopyLength { len: u32, pos: usize },
    BadFinalState,
}

impl fmt::Display for BitKnitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadMagic => write!(f, "missing BitKnit magic"),
            Self::SourceExhausted => write!(f, "compressed stream ended early"),
            Self::BadCopyOffset { offset, pos } => {
                write!(f, "copy offset {offset} before start of output at {pos}")
            }
            Self::BadCopyLength { len, pos } => {
                write!(f, "copy of {len} bytes overruns output at {pos}")
            }
            Self::BadFinalState => write!(f, "rANS state mismatch at end of quantum"),
        }
    }
}

impl std::error::Error for BitKnitError {}

type Result<T> = std::result::Result<T, BitKnitError>;

/// Decompress a complete BitKnit 2 stream into exactly `out_len` bytes.
pub fn decompress(src: &[u8], out_len: usize) -> Result<Vec<u8>> {
    let mut out = vec![0u8; out_len];
    decompress_into(src, &mut out)?;
    Ok(out)
}

/// Decompress a complete BitKnit 2 stream, filling `out` entirely.
pub fn decompress_into(src: &[u8], out: &mut [u8]) -> Result<()> {
    let mut d = Decoder::new(src);
    if d.src.read()? != MAGIC {
        return Err(BitKnitError::BadMagic);
    }
    let mut pos = 0usize;
    while pos < out.len() {
        if d.src.at_end() {
            return Err(BitKnitError::SourceExhausted);
        }
        pos = d.quantum(out, pos)?;
    }
    Ok(())
}

/// Little-endian 16-bit word reader.
struct Words<'a> {
    data: &'a [u8],
    pos: usize,
}

impl Words<'_> {
    fn at_end(&self) -> bool {
        self.pos + 2 > self.data.len()
    }

    fn peek(&self) -> Result<u16> {
        self.data
            .get(self.pos..self.pos + 2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .ok_or(BitKnitError::SourceExhausted)
    }

    fn read(&mut self) -> Result<u16> {
        let w = self.peek()?;
        self.pos += 2;
        Ok(w)
    }
}

/// Adaptive frequency model over `n` symbols with deferred (every 1024 symbols) CDF updates.
struct Model {
    /// Cumulative frequencies, `n + 1` entries, `cdf[n] == 2^15`.
    cdf: Vec<u16>,
    /// Per-symbol counts accumulated since the last update.
    counts: Vec<u16>,
    /// Bucket of the top 10 code bits → first candidate symbol.
    lookup: [u16; 1 << LOOKUP_BITS],
    seen: u32,
    incr: u16,
    last_incr: u16,
}

impl Model {
    /// `n` symbols; the last `min_probable` start at the minimum frequency (1), the rest share
    /// the remaining probability evenly.
    fn new(n: usize, min_probable: usize) -> Self {
        let equi = n - min_probable;
        let total = PROB_TOTAL as usize;
        let mut cdf = vec![0u16; n + 1];
        for (i, c) in cdf.iter_mut().enumerate() {
            *c = if i < equi {
                ((total - min_probable) * i / equi) as u16
            } else {
                (total - n + i) as u16
            };
        }
        let incr = ((PROB_TOTAL - n as u32) / ADAPT_INTERVAL) as u16;
        let last_incr = (1 + PROB_TOTAL - n as u32 - u32::from(incr) * ADAPT_INTERVAL) as u16;
        let mut m = Self {
            cdf,
            counts: vec![1; n],
            lookup: [0; 1 << LOOKUP_BITS],
            seen: 0,
            incr,
            last_incr,
        };
        m.rebuild_lookup();
        m
    }

    fn rebuild_lookup(&mut self) {
        let mut sym = 0usize;
        for (bucket, slot) in self.lookup.iter_mut().enumerate() {
            let code = (bucket << LOOKUP_SHIFT) as u16;
            while self.cdf[sym + 1] <= code {
                sym += 1;
            }
            *slot = sym as u16;
        }
    }

    #[inline]
    fn find(&self, code: u32) -> usize {
        let mut sym = self.lookup[(code >> LOOKUP_SHIFT) as usize] as usize;
        while code >= u32::from(self.cdf[sym + 1]) {
            sym += 1;
        }
        sym
    }

    #[inline]
    fn observe(&mut self, sym: usize) {
        self.counts[sym] += self.incr;
        self.seen += 1;
        if self.seen < ADAPT_INTERVAL {
            return;
        }
        self.seen = 0;
        self.counts[sym] += self.last_incr;
        // Move each CDF entry halfway towards the freshly counted CDF. The difference is taken
        // in unsigned 32-bit and truncated, which the reference decoder relies on.
        let mut sum = 0u32;
        for i in 1..self.cdf.len() {
            sum += u32::from(self.counts[i - 1]);
            let old = self.cdf[i];
            self.cdf[i] = old.wrapping_add((sum.wrapping_sub(u32::from(old)) / 2) as u16);
        }
        self.counts.fill(1);
        self.rebuild_lookup();
    }
}

/// Eight most-recently-used copy offsets. Entries stay in place; `order` holds a permutation in
/// nibbles (nibble `i` = slot of the i-th most recent entry).
struct OffsetCache {
    entries: [u32; 8],
    order: u32,
}

impl OffsetCache {
    fn new() -> Self {
        Self {
            entries: [1; 8],
            order: 0x7654_3210,
        }
    }

    /// Use entry `index` and move it to the front.
    fn hit(&mut self, index: u32) -> u32 {
        let shift = index * 4;
        let slot = (self.order >> shift) & 15;
        let mask = ((16u64 << shift) - 1) as u32;
        let rotated = ((self.order << 4) | slot) & mask;
        self.order = (self.order & !mask) | rotated;
        self.entries[slot as usize]
    }

    /// Insert a new offset in second-to-last position (pushing the old one to last).
    fn insert(&mut self, value: u32) {
        let last = (self.order >> 28) as usize;
        let second_last = ((self.order >> 24) & 15) as usize;
        self.entries[last] = self.entries[second_last];
        self.entries[second_last] = value;
    }
}

struct Decoder<'a> {
    src: Words<'a>,
    commands: [Model; 4],
    cache_refs: [Model; 4],
    offset_lengths: Model,
    cache: OffsetCache,
    last_offset: u32,
    /// The two interleaved rANS states; `state[0]` is the one used next.
    state: [u32; 2],
}

impl<'a> Decoder<'a> {
    fn new(src: &'a [u8]) -> Self {
        Self {
            src: Words { data: src, pos: 0 },
            commands: std::array::from_fn(|_| Model::new(300, 36)),
            cache_refs: std::array::from_fn(|_| Model::new(40, 0)),
            offset_lengths: Model::new(21, 0),
            cache: OffsetCache::new(),
            last_offset: 1,
            state: [RANS_L; 2],
        }
    }

    #[inline]
    fn renorm(src: &mut Words<'_>, x: &mut u32) -> Result<()> {
        if *x < RANS_L {
            *x = (*x << 16) | u32::from(src.read()?);
        }
        Ok(())
    }

    /// Take `n` (< 16) raw bits from the active state, then switch states.
    #[inline]
    fn bits(&mut self, n: u32) -> Result<u32> {
        let x = &mut self.state[0];
        let v = *x & ((1 << n) - 1);
        *x >>= n;
        Self::renorm(&mut self.src, x)?;
        self.state.swap(0, 1);
        Ok(v)
    }

    #[inline]
    fn symbol(src: &mut Words<'_>, state: &mut [u32; 2], model: &mut Model) -> Result<usize> {
        let x = &mut state[0];
        let code = *x & (PROB_TOTAL - 1);
        let sym = model.find(code);
        let lo = u32::from(model.cdf[sym]);
        let freq = u32::from(model.cdf[sym + 1]) - lo;
        *x = (*x >> PROB_BITS) * freq + code - lo;
        Self::renorm(src, x)?;
        model.observe(sym);
        state.swap(0, 1);
        Ok(sym)
    }

    /// Read the per-quantum init block. The final encoder states are packed as: a 4-bit split
    /// count `k` in the low bits, state 1 in the high bits, and the low `16 + k` bits of state 2
    /// (whose top bit is implicit) below it.
    fn init_states(&mut self) -> Result<()> {
        let hi = u32::from(self.src.read()?);
        let lo = u32::from(self.src.read()?);
        let mut merged = (hi << 16) | lo;
        let split = merged & 15;
        merged >>= 4;
        Self::renorm(&mut self.src, &mut merged)?;
        let mut s1 = merged >> split;
        Self::renorm(&mut self.src, &mut s1)?;
        let top = 1u32 << (16 + split);
        let s2 = (((merged << 16) | u32::from(self.src.read()?)) & (top - 1)) | top;
        self.state = [s1, s2];
        Ok(())
    }

    /// Decode one quantum starting at `pos`; returns the new output position.
    fn quantum(&mut self, out: &mut [u8], mut pos: usize) -> Result<usize> {
        let end = out.len().min((pos & !(QUANTUM - 1)) + QUANTUM);
        if self.src.peek()? == 0 {
            // Stored quantum.
            self.src.pos += 2;
            let avail = (self.src.data.len() - self.src.pos) & !1;
            let n = avail.min(end - pos);
            out[pos..pos + n].copy_from_slice(&self.src.data[self.src.pos..self.src.pos + n]);
            self.src.pos += n & !1;
            return Ok(pos + n);
        }
        self.init_states()?;
        if pos == 0 {
            out[0] = self.bits(8)? as u8;
            pos = 1;
        }
        while pos < end {
            let lane = pos % 4;
            let cmd = Self::symbol(&mut self.src, &mut self.state, &mut self.commands[lane])?;
            if cmd < 256 {
                let back = self.last_offset as usize;
                if back > pos {
                    return Err(BitKnitError::BadCopyOffset {
                        offset: self.last_offset,
                        pos,
                    });
                }
                out[pos] = (cmd as u8).wrapping_add(out[pos - back]);
                pos += 1;
            } else {
                pos = self.copy(out, pos, cmd as u32)?;
            }
        }
        if self.state != [RANS_L; 2] {
            return Err(BitKnitError::BadFinalState);
        }
        Ok(pos)
    }

    fn copy(&mut self, out: &mut [u8], pos: usize, cmd: u32) -> Result<usize> {
        let lane = pos % 4;
        let len = if cmd < 288 {
            cmd - 254
        } else {
            let nbits = cmd - 287;
            (1 << nbits) + self.bits(nbits)? + 32
        };
        let r = Self::symbol(&mut self.src, &mut self.state, &mut self.cache_refs[lane])? as u32;
        let offset = if r < 8 {
            self.cache.hit(r)
        } else {
            let nbits =
                Self::symbol(&mut self.src, &mut self.state, &mut self.offset_lengths)? as u32;
            let mut low = self.bits(nbits % 16)?;
            if nbits >= 16 {
                low = (low << 16) | u32::from(self.src.read()?);
            }
            let offset = (32u32 << nbits) + (low << 5) - 32 + (r - 7);
            self.cache.insert(offset);
            offset
        };
        if offset as usize > pos {
            return Err(BitKnitError::BadCopyOffset { offset, pos });
        }
        if len as usize > out.len() - pos {
            return Err(BitKnitError::BadCopyLength { len, pos });
        }
        self.last_offset = offset;
        let from = pos - offset as usize;
        for i in 0..len as usize {
            out[pos + i] = out[from + i];
        }
        Ok(pos + len as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One coded operation, in decoder order.
    enum Op {
        Bits(u32, u32),
        Sym(u32, u32),
    }

    /// A deliberately simple encoder for test fixtures: delta literals plus runs of the
    /// previous byte as copies at cache slot 0 (offset 1, the initial cache content).
    /// Shares the model code with the decoder; it is not Granny's encoder.
    fn encode(data: &[u8]) -> Vec<u8> {
        let mut commands: [Model; 4] = std::array::from_fn(|_| Model::new(300, 36));
        let mut cache_refs: [Model; 4] = std::array::from_fn(|_| Model::new(40, 0));
        let mut out = MAGIC.to_le_bytes().to_vec();
        let sym = |m: &mut Model, s: usize, ops: &mut Vec<Op>| {
            let lo = u32::from(m.cdf[s]);
            ops.push(Op::Sym(lo, u32::from(m.cdf[s + 1]) - lo));
            m.observe(s);
        };
        let mut pos = 0;
        while pos < data.len() {
            let end = data.len().min((pos & !(QUANTUM - 1)) + QUANTUM);
            let mut ops = Vec::new();
            if pos == 0 {
                ops.push(Op::Bits(8, u32::from(data[0])));
                pos = 1;
            }
            while pos < end {
                let lane = pos % 4;
                let run = data[pos..end]
                    .iter()
                    .take(33)
                    .take_while(|&&b| b == data[pos - 1])
                    .count();
                if run >= 2 {
                    sym(&mut commands[lane], 254 + run, &mut ops);
                    sym(&mut cache_refs[lane], 0, &mut ops);
                    pos += run;
                } else {
                    let delta = data[pos].wrapping_sub(data[pos - 1]);
                    sym(&mut commands[lane], usize::from(delta), &mut ops);
                    pos += 1;
                }
            }
            // rANS-encode in reverse; op j uses state j % 2.
            let mut st = [RANS_L; 2];
            let mut emitted: Vec<u16> = Vec::new();
            for (j, op) in ops.iter().enumerate().rev() {
                let x = &mut st[j % 2];
                match *op {
                    Op::Bits(n, v) => {
                        if *x >> (32 - n) != 0 {
                            emitted.push(*x as u16);
                            *x >>= 16;
                        }
                        *x = (*x << n) | v;
                    }
                    Op::Sym(lo, freq) => {
                        if *x >= freq << 17 {
                            emitted.push(*x as u16);
                            *x >>= 16;
                        }
                        *x = ((*x / freq) << PROB_BITS) + *x % freq + lo;
                    }
                }
            }
            let [s1, s2] = st;
            let k = 31 - s2.leading_zeros() - 16;
            let r = s2 - (1 << (16 + k));
            // The decoder unpacks `merged >> k` into state 1 (refilling one word if it is below
            // 2^16) and the low k bits into the top of state 2. Split state 1 when that keeps
            // `merged` >= 2^16, otherwise store it whole. `merged` itself must not start with a
            // zero word (that reads as a stored quantum): use 28 bits directly, or 12 bits plus
            // one refill word.
            let split = ((s1 >> 16) << k) | (r >> 16);
            let (merged, s1_word) = if split >= RANS_L {
                (split, Some(s1 as u16))
            } else {
                let whole = (u64::from(s1) << k) | u64::from(r >> 16);
                (u32::try_from(whole).expect("fixture state too large"), None)
            };
            let mut words = if merged < 1 << 28 {
                let head = (merged << 4) | k;
                vec![(head >> 16) as u16, head as u16]
            } else {
                let head = ((merged >> 16) << 4) | k;
                vec![(head >> 16) as u16, head as u16, merged as u16]
            };
            words.extend(s1_word);
            words.push(r as u16);
            words.extend(emitted.iter().rev());
            for w in words {
                out.extend_from_slice(&w.to_le_bytes());
            }
        }
        out
    }

    #[test]
    fn coded_roundtrip() {
        let mut data = Vec::new();
        let mut x = 12345u32;
        for i in 0..150_000u32 {
            x = x.wrapping_mul(1_103_515_245).wrapping_add(12345);
            // Mix of noise, slow ramps and long runs (to exercise copies and adaptation).
            let b = match (i / 997) % 3 {
                0 => (x >> 24) as u8,
                1 => (i / 3) as u8,
                _ => 7,
            };
            data.push(b);
        }
        let packed = encode(&data);
        assert!(packed.len() < data.len());
        assert_eq!(decompress(&packed, data.len()).unwrap(), data);
        // Corruption is detected (final-state check or bounds).
        let mut bad = packed.clone();
        bad[40] ^= 0x55;
        assert_ne!(decompress(&bad, data.len()).ok(), Some(data));
    }

    #[test]
    fn stored_quanta_roundtrip() {
        // A stream of two stored quanta covering 0x10000 + 5 bytes.
        let payload: Vec<u8> = (0..QUANTUM + 5).map(|i| (i * 7 % 251) as u8).collect();
        let mut s = MAGIC.to_le_bytes().to_vec();
        s.extend_from_slice(&[0, 0]);
        s.extend_from_slice(&payload[..QUANTUM]);
        s.extend_from_slice(&[0, 0]);
        s.extend_from_slice(&payload[QUANTUM..]);
        s.push(0); // pad to a whole word
        assert_eq!(decompress(&s, payload.len()).unwrap(), payload);
    }

    #[test]
    fn rejects_bad_magic_and_truncation() {
        assert_eq!(decompress(&[0, 0, 0, 0], 4), Err(BitKnitError::BadMagic));
        assert_eq!(
            decompress(&MAGIC.to_le_bytes(), 4),
            Err(BitKnitError::SourceExhausted)
        );
        assert!(decompress(&[], 0).is_err());
    }

    #[test]
    fn initial_model_is_valid() {
        for (n, m) in [(300, 36), (40, 0), (21, 0)] {
            let model = Model::new(n, m);
            assert_eq!(model.cdf[0], 0);
            assert_eq!(u32::from(model.cdf[n]), PROB_TOTAL);
            assert!(model.cdf.windows(2).all(|w| w[0] < w[1]));
            for code in (0..PROB_TOTAL).step_by(37) {
                let s = model.find(code);
                assert!(u32::from(model.cdf[s]) <= code && code < u32::from(model.cdf[s + 1]));
            }
        }
    }

    #[test]
    fn offset_cache_lru() {
        let mut c = OffsetCache::new();
        c.insert(10); // goes to second-to-last slot (index 6 in order)
        c.insert(20);
        assert_eq!(c.hit(6), 20);
        // After the hit, 20 is most recent.
        assert_eq!(c.hit(0), 20);
        assert_eq!(c.hit(7), 10);
        assert_eq!(c.hit(0), 10);
    }
}
