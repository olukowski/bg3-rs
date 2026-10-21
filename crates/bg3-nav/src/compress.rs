//! AI data blocks: `u32 inflated size, u32 stored size`, then zlib data that inflates to an LZ4
//! block, which decompresses to the payload (whose size the caller knows).

use std::io::Read;

use crate::bytes::Reader;
use crate::{Error, Result};

// bg3: eoc::LoadCompressedAiData (4.1.1.7631656) — inflate, then LZ4_decompress_safe.
pub fn read_block(r: &mut Reader<'_>, payload_size: usize) -> Result<Vec<u8>> {
    let inflated = r.u32()? as usize;
    let stored = r.u32()? as usize;
    let data = r.take(stored)?;
    if stored == 0 {
        return Ok(Vec::new());
    }
    let mut lz = Vec::with_capacity(inflated);
    flate2::read::ZlibDecoder::new(data)
        .read_to_end(&mut lz)
        .map_err(|e| Error::Decompress(format!("zlib: {e}")))?;
    if lz.len() != inflated {
        return Err(Error::Size {
            what: "inflated block",
            expected: inflated,
            got: lz.len(),
        });
    }
    lz4_flex::block::decompress(&lz, payload_size)
        .map_err(|e| Error::Decompress(format!("lz4: {e}")))
}

/// Inverse of [`read_block`].
pub fn write_block(out: &mut Vec<u8>, payload: &[u8]) {
    use std::io::Write;
    let lz = lz4_flex::block::compress(payload);
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    z.write_all(&lz).expect("write to Vec");
    let stored = z.finish().expect("write to Vec");
    out.extend((lz.len() as u32).to_le_bytes());
    out.extend((stored.len() as u32).to_le_bytes());
    out.extend(stored);
}
