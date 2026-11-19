//! Variable-length integers and the integer bodies of BSUP values.

use crate::error::{Error, Result};

/// Appends the protobuf-style uvarint encoding of `v` to `buf`.
pub(crate) fn put_uvarint(buf: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        buf.push(v as u8 | 0x80);
        v >>= 7;
    }
    buf.push(v as u8);
}

/// A cursor over a byte slice for decoding.
#[derive(Clone, Copy)]
pub(crate) struct Cursor<'a> {
    pub(crate) buf: &'a [u8],
}

impl<'a> Cursor<'a> {
    pub(crate) fn new(buf: &'a [u8]) -> Self {
        Cursor { buf }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    pub(crate) fn uvarint(&mut self) -> Result<u64> {
        let mut v: u64 = 0;
        for (i, &b) in self.buf.iter().enumerate() {
            if i == 9 && b > 1 {
                return Err(Error::malformed("uvarint overflows 64 bits"));
            }
            v |= u64::from(b & 0x7f) << (7 * i);
            if b < 0x80 {
                self.buf = &self.buf[i + 1..];
                return Ok(v);
            }
            if i == 9 {
                break;
            }
        }
        Err(Error::malformed("truncated uvarint"))
    }

    pub(crate) fn uvarint_u32(&mut self) -> Result<u32> {
        let v = self.uvarint()?;
        u32::try_from(v).map_err(|_| Error::malformed("uvarint overflows 32 bits"))
    }

    pub(crate) fn len(&mut self) -> Result<usize> {
        let n = self.uvarint()?;
        if n > self.buf.len() as u64 {
            return Err(Error::malformed("length exceeds input"));
        }
        Ok(n as usize)
    }

    pub(crate) fn bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        if n > self.buf.len() {
            return Err(Error::malformed("truncated input"));
        }
        let (b, rest) = self.buf.split_at(n);
        self.buf = rest;
        Ok(b)
    }

    pub(crate) fn byte(&mut self) -> Result<u8> {
        Ok(self.bytes(1)?[0])
    }

    /// Decodes a counted string.
    pub(crate) fn name(&mut self) -> Result<String> {
        let n = self.len()?;
        let b = self.bytes(n)?;
        String::from_utf8(b.to_vec()).map_err(|_| Error::malformed("name is not valid UTF-8"))
    }

    /// Decodes a length-prefixed value body.
    pub(crate) fn body(&mut self) -> Result<&'a [u8]> {
        let n = self.len()?;
        self.bytes(n)
    }
}

/// Appends a counted string.
pub(crate) fn put_name(buf: &mut Vec<u8>, name: &str) {
    put_uvarint(buf, name.len() as u64);
    buf.extend_from_slice(name.as_bytes());
}

/// Appends a length-prefixed body.
pub(crate) fn put_body(buf: &mut Vec<u8>, body: &[u8]) {
    put_uvarint(buf, body.len() as u64);
    buf.extend_from_slice(body);
}

/// Appends the body of an unsigned integer: its minimal little-endian bytes.
pub(crate) fn put_uint(buf: &mut Vec<u8>, mut v: u64) {
    while v != 0 {
        buf.push(v as u8);
        v >>= 8;
    }
}

/// Appends the body of a signed integer.
pub(crate) fn put_int(buf: &mut Vec<u8>, v: i64) {
    put_uint(buf, zigzag(v));
}

pub(crate) fn zigzag(v: i64) -> u64 {
    if v >= 0 {
        (v as u64) << 1
    } else {
        (v.wrapping_neg() as u64).wrapping_shl(1) | 1
    }
}

pub(crate) fn unzigzag(u: u64) -> i64 {
    if u & 1 == 0 {
        (u >> 1) as i64
    } else if u >> 1 == 0 {
        i64::MIN
    } else {
        ((u >> 1) as i64).wrapping_neg()
    }
}

/// Decodes the body of an unsigned integer.
pub(crate) fn decode_uint(body: &[u8]) -> Result<u64> {
    if body.len() > 8 {
        return Err(Error::malformed("integer body longer than 8 bytes"));
    }
    let mut v = 0u64;
    for (i, &b) in body.iter().enumerate() {
        v |= u64::from(b) << (8 * i);
    }
    Ok(v)
}

/// Decodes the body of a signed integer.
pub(crate) fn decode_int(body: &[u8]) -> Result<i64> {
    Ok(unzigzag(decode_uint(body)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn int_bodies() {
        for v in [0i64, 1, -1, 63, -64, 1000, i64::MAX, i64::MIN] {
            let mut b = Vec::new();
            put_int(&mut b, v);
            assert_eq!(decode_int(&b).unwrap(), v);
        }
        let mut b = Vec::new();
        put_int(&mut b, i64::MIN);
        assert_eq!(b, vec![1]);
        let mut b = Vec::new();
        put_uint(&mut b, 0);
        assert!(b.is_empty());
    }

    #[test]
    fn uvarints() {
        for v in [0u64, 1, 127, 128, 300, u32::MAX as u64, u64::MAX] {
            let mut b = Vec::new();
            put_uvarint(&mut b, v);
            assert_eq!(Cursor::new(&b).uvarint().unwrap(), v);
        }
        assert!(Cursor::new(&[0x80]).uvarint().is_err());
    }
}
