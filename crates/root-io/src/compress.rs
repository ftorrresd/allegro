//! ROOT's compressed-block container.
//!
//! A compressed record is a sequence of independent blocks, each introduced by
//! a nine-byte header:
//!
//! ```text
//! [0..2]  two-letter algorithm tag  ("ZL", "XZ", "L4", "ZS", "CS")
//! [2]     algorithm-specific method/level
//! [3..6]  compressed size,   three bytes, little-endian
//! [6..9]  uncompressed size, three bytes, little-endian
//! ```
//!
//! Because the sizes are 24-bit, ROOT splits large payloads into several
//! blocks, which are simply concatenated once decompressed.

use std::io::Read;

use crate::error::{Error, Result};

const HEADER_LEN: usize = 9;

fn u24_le(b: &[u8]) -> usize {
    b[0] as usize | (b[1] as usize) << 8 | (b[2] as usize) << 16
}

/// Decompresses `data` until `expected` bytes have been produced.
pub fn decompress(data: &[u8], expected: usize) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(expected);
    let mut pos = 0usize;

    while out.len() < expected {
        if pos + HEADER_LEN > data.len() {
            return Err(Error::format(format!(
                "compressed record ended after {} of {} bytes",
                out.len(),
                expected
            )));
        }
        let header = &data[pos..pos + HEADER_LEN];
        let tag = [header[0], header[1]];
        let csize = u24_le(&header[3..6]);
        let usize_ = u24_le(&header[6..9]);
        let body_start = pos + HEADER_LEN;
        let body_end = body_start + csize;
        if body_end > data.len() {
            return Err(Error::format(format!(
                "compressed block claims {csize} bytes but only {} remain",
                data.len() - body_start
            )));
        }
        let body = &data[body_start..body_end];

        let chunk = match &tag {
            b"ZL" => miniz_oxide::inflate::decompress_to_vec_zlib(body)
                .map_err(|e| Error::format(format!("zlib block: {e:?}")))?,
            b"XZ" => {
                let mut decoded = Vec::with_capacity(usize_);
                let mut reader = std::io::BufReader::new(body);
                lzma_rs::xz_decompress(&mut reader, &mut decoded)
                    .map_err(|e| Error::format(format!("lzma block: {e}")))?;
                decoded
            }
            b"L4" => {
                // An 8-byte xxhash64 checksum precedes the LZ4 block.
                if body.len() < 8 {
                    return Err(Error::format("lz4 block too short for its checksum"));
                }
                lz4_flex::block::decompress(&body[8..], usize_)
                    .map_err(|e| Error::format(format!("lz4 block: {e}")))?
            }
            b"ZS" => {
                let mut decoder = ruzstd::StreamingDecoder::new(body)
                    .map_err(|e| Error::format(format!("zstd block: {e}")))?;
                let mut decoded = Vec::with_capacity(usize_);
                decoder
                    .read_to_end(&mut decoded)
                    .map_err(|e| Error::format(format!("zstd block: {e}")))?;
                decoded
            }
            other => {
                return Err(Error::format(format!(
                    "unsupported ROOT compression algorithm {:?}",
                    String::from_utf8_lossy(other)
                )))
            }
        };

        if chunk.len() != usize_ {
            return Err(Error::format(format!(
                "block declared {usize_} bytes but produced {}",
                chunk.len()
            )));
        }
        out.extend_from_slice(&chunk);
        pos = body_end;
    }

    out.truncate(expected);
    Ok(out)
}

/// Compresses with zlib in ROOT's block format.
///
/// Used when writing output files; a single block suffices for the record
/// sizes this tool produces, and 24-bit sizes cap a block at 16 MiB.
pub fn compress_zlib(data: &[u8], level: u8) -> Result<Vec<u8>> {
    const MAX_BLOCK: usize = 0xFF_FFFF;
    if data.len() > MAX_BLOCK {
        return Err(Error::format(format!(
            "record of {} bytes exceeds the single-block limit",
            data.len()
        )));
    }
    let body = miniz_oxide::deflate::compress_to_vec_zlib(data, level);
    if body.len() > MAX_BLOCK {
        return Err(Error::format(
            "compressed block exceeds the 24-bit size field",
        ));
    }
    let mut out = Vec::with_capacity(HEADER_LEN + body.len());
    out.extend_from_slice(b"ZL");
    out.push(8); // Z_DEFLATED
    out.extend_from_slice(&[
        (body.len() & 0xff) as u8,
        ((body.len() >> 8) & 0xff) as u8,
        ((body.len() >> 16) & 0xff) as u8,
    ]);
    out.extend_from_slice(&[
        (data.len() & 0xff) as u8,
        ((data.len() >> 8) & 0xff) as u8,
        ((data.len() >> 16) & 0xff) as u8,
    ]);
    out.extend_from_slice(&body);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zlib_blocks_round_trip() {
        let payload: Vec<u8> = (0..5000u32).map(|i| (i % 251) as u8).collect();
        let packed = compress_zlib(&payload, 6).unwrap();
        assert_eq!(&packed[..2], b"ZL");
        let back = decompress(&packed, payload.len()).unwrap();
        assert_eq!(back, payload);
    }

    #[test]
    fn a_truncated_block_is_an_error() {
        let payload = vec![7u8; 100];
        let mut packed = compress_zlib(&payload, 6).unwrap();
        packed.truncate(packed.len() - 5);
        assert!(decompress(&packed, payload.len()).is_err());
    }
}
