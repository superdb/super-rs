//! Decompression of LZ4 blocks (see the LZ4 block format at
//! <https://github.com/lz4/lz4/blob/master/doc/lz4_Block_format.md>).

use crate::error::{Error, Result};

/// Decompresses the LZ4 block `src`, whose decompressed length is `n`.
pub(crate) fn decompress(src: &[u8], n: usize) -> Result<Vec<u8>> {
    let bad = || Error::malformed("bad LZ4 block");
    let mut out = Vec::with_capacity(n);
    let mut i = 0;
    while i < src.len() {
        let token = src[i];
        i += 1;
        let mut lit = usize::from(token >> 4);
        if lit == 15 {
            loop {
                let b = *src.get(i).ok_or_else(bad)?;
                i += 1;
                lit += usize::from(b);
                if b != 255 {
                    break;
                }
            }
        }
        let end = i
            .checked_add(lit)
            .filter(|&e| e <= src.len())
            .ok_or_else(bad)?;
        if out.len() + lit > n {
            return Err(bad());
        }
        out.extend_from_slice(&src[i..end]);
        i = end;
        if i == src.len() {
            // The last sequence has literals only.
            break;
        }
        if i + 2 > src.len() {
            return Err(bad());
        }
        let offset = usize::from(u16::from_le_bytes([src[i], src[i + 1]]));
        i += 2;
        if offset == 0 || offset > out.len() {
            return Err(bad());
        }
        let mut len = usize::from(token & 0xf);
        if len == 15 {
            loop {
                let b = *src.get(i).ok_or_else(bad)?;
                i += 1;
                len += usize::from(b);
                if b != 255 {
                    break;
                }
            }
        }
        len += 4;
        if out.len() + len > n {
            return Err(bad());
        }
        let start = out.len() - offset;
        if offset >= len {
            out.extend_from_within(start..start + len);
        } else {
            for k in 0..len {
                let b = out[start + k];
                out.push(b);
            }
        }
    }
    if out.len() != n {
        return Err(Error::malformed(format!(
            "LZ4 block decompressed to {} bytes, expected {n}",
            out.len()
        )));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literals_and_matches() {
        // "abcabcabcabcX": 3 literals, a match of 9 at offset 3, then 1
        // literal.
        let src = [0x35, b'a', b'b', b'c', 3, 0, 0x10, b'X'];
        assert_eq!(decompress(&src, 13).unwrap(), b"abcabcabcabcX");
        assert!(decompress(&src, 12).is_err());
        assert!(decompress(&[0x10, b'a', 0, 0], 5).is_err());
    }
}
