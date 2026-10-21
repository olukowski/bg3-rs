//! FastLZ decompression (the "lz77" / "fastlz0.1.0" tile codec).
//!
//! FastLZ is Ariya Hidayat's byte-oriented LZ77 (MIT). The stream is a sequence of instructions,
//! each starting with a control byte; the top three bits of the *first* byte select the format
//! level (0 → level 1, 1 → level 2) and are otherwise ignored.
//!
//! - control `< 32`: literal run of `control + 1` bytes.
//! - control `>= 32`: back-reference. `len = (control >> 5) + 2` bytes are copied from
//!   `distance + 1` bytes back, where `distance = (control & 31) << 8 | next byte`. A length field
//!   of 7 means the length continues in following byte(s): one extra byte in level 1, a run of
//!   bytes terminated by a value `< 255` in level 2. Level 2 additionally escapes long distances:
//!   a low byte of 255 with high bits 31 is followed by a big-endian u16 distance offset by 8191.
//!
//! Written from the format description; no code taken from the reference implementation.

/// Distance bias of level-2 "far" matches.
const FAR_DISTANCE_BIAS: usize = 8191;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FastLzError {
    #[error("unknown FastLZ level {0}")]
    Level(u8),
    #[error("truncated FastLZ stream")]
    Truncated,
    #[error("FastLZ back-reference before start of output")]
    BadDistance,
    #[error("FastLZ output exceeds the expected {0} bytes")]
    Overflow(usize),
}

/// Decompress a FastLZ stream. `max_output` bounds the output size (corrupt input guard).
pub fn decompress(input: &[u8], max_output: usize) -> Result<Vec<u8>, FastLzError> {
    let mut out = Vec::with_capacity(max_output);
    let Some(&first) = input.first() else {
        return Ok(out);
    };
    let level = (first >> 5) + 1;
    if level > 2 {
        return Err(FastLzError::Level(level));
    }

    let mut ip = 1;
    let mut ctrl = (first & 31) as usize;
    let next = |ip: &mut usize| -> Result<usize, FastLzError> {
        let b = *input.get(*ip).ok_or(FastLzError::Truncated)?;
        *ip += 1;
        Ok(b as usize)
    };

    loop {
        if ctrl < 32 {
            let n = ctrl + 1;
            let lit = input.get(ip..ip + n).ok_or(FastLzError::Truncated)?;
            if out.len() + n > max_output {
                return Err(FastLzError::Overflow(max_output));
            }
            out.extend_from_slice(lit);
            ip += n;
        } else {
            let mut len = (ctrl >> 5) - 1;
            let high = (ctrl & 31) << 8;
            if len == 6 {
                if level == 1 {
                    len += next(&mut ip)?;
                } else {
                    loop {
                        let code = next(&mut ip)?;
                        len += code;
                        if code != 255 {
                            break;
                        }
                    }
                }
            }
            let low = next(&mut ip)?;
            len += 3;
            let mut back = high + low + 1;
            if level == 2 && low == 255 && high == 31 << 8 {
                let far = (next(&mut ip)? << 8) | next(&mut ip)?;
                back = far + FAR_DISTANCE_BIAS + 1;
            }
            let start = out
                .len()
                .checked_sub(back)
                .ok_or(FastLzError::BadDistance)?;
            if out.len() + len > max_output {
                return Err(FastLzError::Overflow(max_output));
            }
            // Byte-by-byte: the source may overlap the bytes being written (run-length copies).
            for i in 0..len {
                out.push(out[start + i]);
            }
        }
        if ip >= input.len() {
            break;
        }
        ctrl = next(&mut ip)?;
    }
    Ok(out)
}
