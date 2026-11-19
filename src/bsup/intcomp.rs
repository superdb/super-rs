//! Decoding of the integer segments of column frames, which are compressed
//! with the codec of github.com/ronanh/intcomp (`CompressUint32`,
//! `CompressInt64`, and `CompressUint64`): delta bit-packing of blocks of
//! 128 (32-bit) or 256 (64-bit) integers, with any remainder delta-encoded
//! as variable-length bytes.

use crate::error::{Error, Result};

const BLOCK32: usize = 128;
const BLOCK64: usize = 256;

fn bad() -> Error {
    Error::malformed("bad compressed integer segment")
}

/// Decodes a segment of unsigned 32-bit integers.
pub(crate) fn uint32s(bytes: &[u8]) -> Result<Vec<u32>> {
    if bytes.len() % 4 != 0 {
        return Err(bad());
    }
    let words: Vec<u32> = bytes
        .chunks_exact(4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect();
    let mut out = Vec::new();
    // The last word is the compressed length of the last block.
    let mut input = match words.split_last() {
        Some((_, rest)) => rest,
        None => return Ok(out),
    };
    while !input.is_empty() {
        input = if (input[0] as usize) < BLOCK32 {
            varbyte32(input, &mut out)?
        } else {
            binpack32(input, &mut out)?
        };
    }
    Ok(out)
}

/// Decodes a segment of signed 64-bit integers.
pub(crate) fn int64s(bytes: &[u8]) -> Result<Vec<i64>> {
    Ok(words64(bytes, true)?
        .into_iter()
        .map(|v| v as i64)
        .collect())
}

/// Decodes a segment of unsigned 64-bit integers.
pub(crate) fn uint64s(bytes: &[u8]) -> Result<Vec<u64>> {
    words64(bytes, false)
}

fn words64(bytes: &[u8], signed: bool) -> Result<Vec<u64>> {
    if bytes.len() % 8 != 0 {
        return Err(bad());
    }
    let words: Vec<u64> = bytes
        .chunks_exact(8)
        .map(|b| u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]))
        .collect();
    let mut out = Vec::new();
    let mut input = match words.split_last() {
        Some((_, rest)) => rest,
        None => return Ok(out),
    };
    while !input.is_empty() {
        let n = input[0] as u32 as i32;
        input = if n >= 0 && (n as usize) < BLOCK64 {
            varbyte64(input, &mut out, signed)?
        } else {
            binpack64(input, &mut out, signed)?
        };
    }
    Ok(out)
}

/// Decodes a delta variable-byte block of 32-bit integers and returns the
/// input that follows it.
fn varbyte32<'a>(input: &'a [u32], out: &mut Vec<u32>) -> Result<&'a [u32]> {
    let outlen = input[0] as usize;
    let inlen = *input.get(1).ok_or_else(bad)? as usize;
    if inlen < 2 || inlen > input.len() {
        return Err(bad());
    }
    let (block, rest) = input.split_at(inlen);
    let start = out.len();
    let mut prev = 0u32;
    let mut delta = 0u32;
    let mut shift = 0u32;
    for &w in &block[2..] {
        for k in (0..4).rev() {
            let c = (w >> (8 * k)) & 0xff;
            if shift < 32 {
                delta = delta.wrapping_add((c & 0x7f) << shift);
            }
            shift += 7;
            if c & 0x80 == 0 {
                prev = delta.wrapping_add(prev);
                out.push(prev);
                delta = 0;
                shift = 0;
            }
        }
    }
    if out.len() - start != outlen {
        return Err(bad());
    }
    Ok(rest)
}

fn varbyte64<'a>(input: &'a [u64], out: &mut Vec<u64>, _signed: bool) -> Result<&'a [u64]> {
    let outlen = input[0] as u32 as usize;
    let inlen = (input[0] >> 32) as usize;
    if inlen < 1 || inlen > input.len() {
        return Err(bad());
    }
    let (block, rest) = input.split_at(inlen);
    let start = out.len();
    let mut prev = 0u64;
    let mut delta = 0u64;
    let mut shift = 0u32;
    for &w in &block[1..] {
        for k in (0..8).rev() {
            let c = (w >> (8 * k)) & 0xff;
            if shift < 64 {
                delta = delta.wrapping_add((c & 0x7f) << shift);
            }
            shift += 7;
            if c & 0x80 == 0 {
                prev = delta.wrapping_add(prev);
                out.push(prev);
                delta = 0;
                shift = 0;
            }
        }
    }
    if out.len() - start != outlen {
        return Err(bad());
    }
    Ok(rest)
}

/// Returns the `width`-bit integer at index `i` of the bit-packed `words`
/// of `bits`-bit words, packed least-significant bit first.
fn extract(words: &[u64], bits: u32, width: u32, i: usize) -> u64 {
    let off = i as u64 * u64::from(width);
    let word = (off / u64::from(bits)) as usize;
    let shift = (off % u64::from(bits)) as u32;
    let mut v = words[word] >> shift;
    if shift + width > bits {
        v |= words[word + 1] << (bits - shift);
    }
    let mask = if width >= 64 {
        u64::MAX
    } else {
        (1u64 << width) - 1
    };
    v & mask & if bits == 32 { 0xffff_ffff } else { u64::MAX }
}

fn binpack32<'a>(input: &'a [u32], out: &mut Vec<u32>) -> Result<&'a [u32]> {
    if input.len() < 3 {
        return Err(bad());
    }
    let n = input[0] as usize;
    if n % BLOCK32 != 0 {
        return Err(bad());
    }
    let mut prev = input[2];
    let mut pos = 3;
    let mut group = [0u64; 32];
    for _ in 0..n / BLOCK32 {
        let header = *input.get(pos).ok_or_else(bad)?;
        pos += 1;
        for g in 0..4 {
            let shift = 24 - 8 * g;
            let sign = (header >> (shift + 7)) & 1 == 1;
            let width = (header >> shift) & 0x7f;
            if width > 32 {
                return Err(bad());
            }
            let words = input.get(pos..pos + width as usize).ok_or_else(bad)?;
            pos += width as usize;
            if width == 32 {
                // The values are stored as is.
                out.extend_from_slice(words);
                prev = words[31];
                continue;
            }
            for (k, w) in words.iter().enumerate() {
                group[k] = u64::from(*w);
            }
            for i in 0..32 {
                let v = if width == 0 {
                    0
                } else {
                    extract(&group, 32, width, i) as u32
                };
                let d = if sign {
                    (v >> 1) ^ (v & 1).wrapping_neg()
                } else {
                    v
                };
                prev = prev.wrapping_add(d);
                out.push(prev);
            }
        }
    }
    Ok(&input[pos..])
}

fn binpack64<'a>(input: &'a [u64], out: &mut Vec<u64>, signed: bool) -> Result<&'a [u64]> {
    if input.len() < 2 {
        return Err(bad());
    }
    let n = input[0] as u32 as usize;
    if n % BLOCK64 != 0 {
        return Err(bad());
    }
    let mut prev = input[1];
    let mut pos = 2;
    for _ in 0..n / BLOCK64 {
        let header = *input.get(pos).ok_or_else(bad)?;
        pos += 1;
        for g in 0..4u32 {
            let shift = 24 - 8 * g;
            let sign = (header >> (shift + 7)) & 1 == 1;
            let bitlen = ((header >> shift) & 0x7f) as u32;
            // Only the signed codec trims trailing zeros.
            let ntz = if signed {
                ((header >> (56 - 8 * g)) & 0xff) as u32
            } else {
                0
            };
            let width = bitlen
                .checked_sub(ntz)
                .filter(|&w| w <= 64)
                .ok_or_else(bad)?;
            if ntz >= 64 {
                return Err(bad());
            }
            let words = input.get(pos..pos + width as usize).ok_or_else(bad)?;
            pos += width as usize;
            if width == 64 {
                out.extend_from_slice(words);
                prev = words[63];
                continue;
            }
            for i in 0..64 {
                let v = if width == 0 {
                    0
                } else {
                    extract(words, 64, width, i)
                };
                let d = if sign {
                    (v >> 1) ^ (v & 1).wrapping_neg()
                } else {
                    v
                };
                prev = prev.wrapping_add(d << ntz);
                out.push(prev);
            }
        }
    }
    Ok(&input[pos..])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes32(words: &[u32]) -> Vec<u8> {
        words.iter().flat_map(|w| w.to_le_bytes()).collect()
    }

    #[test]
    fn varbyte() {
        // The encoding of [1, 2, 300] by CompressUint32: a varbyte block of
        // 3 values (deltas 1, 1, 298) in 3 words, then the block length.
        // 298 = 0b10_0101010 is the bytes 0xaa, 0x02.
        let block = [3u32, 3, 0x0101_aa02, 3];
        assert_eq!(uint32s(&bytes32(&block)).unwrap(), vec![1, 2, 300]);
        assert!(uint32s(&[]).unwrap().is_empty());
    }

    #[test]
    fn extract_bits() {
        // Three 5-bit values packed into one 32-bit word.
        let w = 3 | (17 << 5) | (31 << 10);
        assert_eq!(extract(&[w, 0], 32, 5, 0), 3);
        assert_eq!(extract(&[w, 0], 32, 5, 1), 17);
        assert_eq!(extract(&[w, 0], 32, 5, 2), 31);
        // A value straddling two words.
        let words = [0xf000_0000u64, 0x1];
        assert_eq!(
            extract(&words, 32, 5, 5),
            ((0xf000_0000u64 >> 25) | (1 << 7)) & 0x1f
        );
    }
}
