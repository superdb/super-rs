//! Decoding of column frames.
//!
//! The locators of a column frame describe its vectors in depth-first order
//! of their types, and the segments of the vectors are laid out in the data
//! section in the same order, so the decoder traverses the locators once,
//! reading each segment as it's located, and builds the values of each
//! vector bottom up.

use std::sync::Arc;

use super::{intcomp, lz4};
use crate::error::{Error, Result};
use crate::typedefs::TypeDefs;
use crate::types::{Primitive, Type};
use crate::value::Value;
use crate::varint::Cursor;

const KIND_PLAIN: u8 = 0;
const KIND_CONST: u8 = 1;
const KIND_DICT: u8 = 2;
const KIND_NONE: u8 = 3;

/// Bounds the nesting of vectors.  (SuperDB allows 10,000 but this decoder
/// is recursive, so it uses a lower bound to stay within the stack.)
const MAX_DEPTH: usize = 1000;

/// Decodes the values of a column frame.
pub(crate) fn decode_column_frame(
    fused: u32,
    vector_typedefs: &[u8],
    locators: &[u8],
    type_values: &[u8],
    data: &[u8],
) -> Result<Vec<Value>> {
    let vtypes = TypeDefs::decode(vector_typedefs)?;
    let tvtypes = TypeDefs::decode(type_values)?;
    tvtypes.lookup(fused)?;
    let mut d = Decoder {
        vtypes,
        tvtypes,
        loc: Cursor::new(locators),
        data,
        off: 0,
        depth: 0,
    };
    let n = d.loc.uvarint()?;
    if n == 0 || n > locators.len() as u64 {
        return Err(Error::malformed("bad number of root vectors"));
    }
    let mut ids = Vec::with_capacity(n as usize);
    for _ in 0..n {
        ids.push(d.loc.uvarint_u32()?);
    }
    let tags = if n > 1 { Some(d.uint32s()?) } else { None };
    let mut roots = Vec::with_capacity(ids.len());
    for &id in &ids {
        let ty = d.vtypes.lookup(id)?;
        roots.push(d.vector(&ty, Place::Vector)?);
    }
    if !d.loc.is_empty() {
        return Err(Error::malformed("extra bytes in locator section"));
    }
    if d.off != data.len() {
        return Err(Error::malformed("segments don't fill the data section"));
    }
    match tags {
        None => Ok(roots.pop().unwrap()),
        Some(tags) => interleave(&tags, roots),
    }
}

/// Returns the values of `vecs` in the order given by `tags`, which select
/// the vector of each value.
fn interleave(tags: &[u32], vecs: Vec<Vec<Value>>) -> Result<Vec<Value>> {
    let total: usize = vecs.iter().map(Vec::len).sum();
    if tags.len() != total {
        return Err(Error::malformed(
            "tags don't match the lengths of the vectors",
        ));
    }
    let mut iters: Vec<_> = vecs.into_iter().map(Vec::into_iter).collect();
    let mut out = Vec::with_capacity(total);
    for &t in tags {
        let v = iters
            .get_mut(t as usize)
            .and_then(Iterator::next)
            .ok_or_else(|| Error::malformed("bad tag"))?;
        out.push(v);
    }
    Ok(out)
}

#[derive(Clone, Copy, PartialEq)]
enum Place {
    Vector,
    Option,
    Dict,
}

struct Decoder<'a> {
    vtypes: TypeDefs,
    tvtypes: TypeDefs,
    loc: Cursor<'a>,
    data: &'a [u8],
    off: usize,
    depth: usize,
}

impl<'a> Decoder<'a> {
    /// Reads the next segment.
    fn segment(&mut self) -> Result<Vec<u8>> {
        let format = self.loc.byte()?;
        let mem_len = self.loc.uvarint()?;
        let (file_len, compressed) = match format {
            0 => (mem_len, false),
            1 => (self.loc.uvarint()?, true),
            _ => {
                return Err(Error::unsupported(format!(
                    "segment compression format {format}"
                )))
            }
        };
        let end = (self.off as u64)
            .checked_add(file_len)
            .filter(|&e| e <= self.data.len() as u64)
            .ok_or_else(|| Error::malformed("segment is past the end of the data section"))?
            as usize;
        let bytes = &self.data[self.off..end];
        self.off = end;
        if compressed {
            if mem_len > 256 * file_len + 256 {
                return Err(Error::malformed("bad compressed segment length"));
            }
            lz4::decompress(bytes, mem_len as usize)
        } else {
            Ok(bytes.to_vec())
        }
    }

    fn uint32s(&mut self) -> Result<Vec<u32>> {
        let b = self.segment()?;
        intcomp::uint32s(&b)
    }

    fn count(&mut self) -> Result<usize> {
        Ok(self.loc.uvarint_u32()? as usize)
    }

    /// Decodes the vector of type `id` whose locators are next.
    fn vector(&mut self, ty: &Type, place: Place) -> Result<Vec<Value>> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(Error::malformed("vectors nested too deeply"));
        }
        let out = self.vector1(ty, place);
        self.depth -= 1;
        out
    }

    fn vector1(&mut self, ty: &Type, place: Place) -> Result<Vec<Value>> {
        let kind = self.loc.byte()?;
        if (kind == KIND_NONE && place != Place::Option)
            || (kind != KIND_PLAIN && place == Place::Dict)
        {
            return Err(Error::malformed(format!(
                "vector kind {kind} not allowed here"
            )));
        }
        match kind {
            KIND_CONST => {
                let n = self.count()?;
                let len = self.loc.uvarint()?;
                let v = if len == 0 {
                    Value::Null
                } else {
                    let body = self.loc.bytes((len - 1) as usize)?;
                    Value::decode(ty, body)?
                };
                Ok(vec![v; n])
            }
            KIND_NONE => {
                let n = self.count()?;
                Ok(vec![Value::None; n])
            }
            KIND_DICT => {
                if !matches!(ty, Type::Primitive(_)) {
                    return Err(Error::malformed("dict vector of a complex type"));
                }
                let values = self.vector(ty, Place::Dict)?;
                let _counts = self.uint32s()?;
                let index = self.segment()?;
                index
                    .iter()
                    .map(|&k| {
                        values
                            .get(usize::from(k))
                            .cloned()
                            .ok_or_else(|| Error::malformed("bad dict index"))
                    })
                    .collect()
            }
            KIND_PLAIN => match ty {
                Type::Primitive(p) => self.primitive(*p),
                _ => self.complex(ty),
            },
            _ => Err(Error::malformed(format!("unknown vector kind {kind}"))),
        }
    }

    fn primitive(&mut self, p: Primitive) -> Result<Vec<Value>> {
        use Primitive as P;
        let n = self.count()?;
        let check = |len: usize| -> Result<()> {
            if len != n {
                Err(Error::malformed(format!(
                    "{} vector has {len} values, expected {n}",
                    p.name()
                )))
            } else {
                Ok(())
            }
        };
        let range = || Error::malformed("integer out of range for its type");
        Ok(match p {
            P::Uint8 | P::Uint16 | P::Uint32 | P::Uint64 => {
                let vals = intcomp::uint64s(&self.segment()?)?;
                check(vals.len())?;
                vals.into_iter()
                    .map(|v| {
                        Ok(match p {
                            P::Uint8 => Value::Uint8(u8::try_from(v).map_err(|_| range())?),
                            P::Uint16 => Value::Uint16(u16::try_from(v).map_err(|_| range())?),
                            P::Uint32 => Value::Uint32(u32::try_from(v).map_err(|_| range())?),
                            _ => Value::Uint64(v),
                        })
                    })
                    .collect::<Result<_>>()?
            }
            P::Int8 | P::Int16 | P::Int32 | P::Int64 | P::Duration | P::Timestamp => {
                let vals = intcomp::int64s(&self.segment()?)?;
                check(vals.len())?;
                vals.into_iter()
                    .map(|v| {
                        Ok(match p {
                            P::Int8 => Value::Int8(i8::try_from(v).map_err(|_| range())?),
                            P::Int16 => Value::Int16(i16::try_from(v).map_err(|_| range())?),
                            P::Int32 => Value::Int32(i32::try_from(v).map_err(|_| range())?),
                            P::Int64 => Value::Int64(v),
                            P::Duration => Value::Duration(v),
                            _ => Value::Timestamp(v),
                        })
                    })
                    .collect::<Result<_>>()?
            }
            P::Float16 | P::Float32 | P::Float64 => {
                let b = self.segment()?;
                if b.len() != 8 * n {
                    return Err(Error::malformed("bad float segment length"));
                }
                b.chunks_exact(8)
                    .map(|c| {
                        let v =
                            f64::from_le_bytes([c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]]);
                        match p {
                            P::Float16 => Value::Float16(v as f32),
                            P::Float32 => Value::Float32(v as f32),
                            _ => Value::Float64(v),
                        }
                    })
                    .collect()
            }
            P::Bool => {
                let b = self.segment()?;
                if b.len() * 8 < n {
                    return Err(Error::malformed("short bool segment"));
                }
                (0..n)
                    .map(|k| Value::Bool(b[k / 8] >> (k % 8) & 1 == 1))
                    .collect()
            }
            P::Bytes | P::String | P::Uuid | P::Ip | P::Net | P::All => {
                let bodies = self.bytes_table(n)?;
                bodies
                    .into_iter()
                    .map(|b| match p {
                        P::All => Ok(Value::Bytes(b)),
                        // SuperDB writes the networks of a net vector as an
                        // address followed by a 1-byte prefix length (the
                        // binary form of Go's netip.Prefix) rather than as
                        // value bodies.
                        P::Net if b.len() == 5 || b.len() == 17 => {
                            let (addr, prefix) = b.split_at(b.len() - 1);
                            let ip = match Value::decode(&Type::IP, addr)? {
                                Value::Ip(ip) => ip,
                                _ => unreachable!(),
                            };
                            if usize::from(prefix[0]) > addr.len() * 8 {
                                return Err(Error::malformed("bad net prefix length"));
                            }
                            Ok(Value::Net(ip, prefix[0]))
                        }
                        _ => Value::decode(&Type::Primitive(p), &b),
                    })
                    .collect::<Result<_>>()?
            }
            P::Type => {
                let ids = self.uint32s()?;
                check(ids.len())?;
                ids.into_iter()
                    .map(|id| Ok(Value::Type(self.tvtypes.lookup(id)?)))
                    .collect::<Result<_>>()?
            }
            P::Null => vec![Value::Null; n],
            P::Missing => vec![Value::Missing; n],
            P::None => vec![Value::None; n],
            _ => {
                return Err(Error::malformed(format!(
                    "type {} has no vector encoding",
                    p.name()
                )))
            }
        })
    }

    fn bytes_table(&mut self, n: usize) -> Result<Vec<Vec<u8>>> {
        let bytes = self.segment()?;
        let offsets = self.uint32s()?;
        if n == 0 {
            return Ok(Vec::new());
        }
        if offsets.len() != n + 1 || offsets[0] != 0 {
            return Err(Error::malformed("bad bytes table offsets"));
        }
        let mut out = Vec::with_capacity(n);
        for w in offsets.windows(2) {
            let (a, b) = (w[0] as usize, w[1] as usize);
            if a > b || b > bytes.len() {
                return Err(Error::malformed("bad bytes table offsets"));
            }
            out.push(bytes[a..b].to_vec());
        }
        Ok(out)
    }

    /// Splits `values` into `n` containers by `offsets`.
    fn split<T>(offsets: &[u32], n: usize, values: Vec<T>) -> Result<Vec<Vec<T>>> {
        if n == 0 {
            if !values.is_empty() {
                return Err(Error::malformed("values of an empty container vector"));
            }
            return Ok(Vec::new());
        }
        if offsets.len() != n + 1 || offsets[0] != 0 || offsets[n] as usize != values.len() {
            return Err(Error::malformed("bad container offsets"));
        }
        let mut it = values.into_iter();
        let mut out = Vec::with_capacity(n);
        for w in offsets.windows(2) {
            if w[0] > w[1] {
                return Err(Error::malformed("bad container offsets"));
            }
            out.push(it.by_ref().take((w[1] - w[0]) as usize).collect());
        }
        Ok(out)
    }

    fn complex(&mut self, ty: &Type) -> Result<Vec<Value>> {
        match ty {
            Type::Record(fields) => {
                if fields.is_empty() {
                    let n = self.count()?;
                    return Ok(vec![Value::Record(Vec::new()); n]);
                }
                let mut cols = Vec::with_capacity(fields.len());
                for f in fields.iter() {
                    cols.push(self.vector(&f.ty, Place::Vector)?.into_iter());
                }
                let n = cols[0].len();
                if cols.iter().any(|c| c.len() != n) {
                    return Err(Error::malformed("record fields have different lengths"));
                }
                let mut out = Vec::with_capacity(n);
                for _ in 0..n {
                    let rec = fields
                        .iter()
                        .zip(cols.iter_mut())
                        .map(|(f, c)| (f.name.clone(), c.next().unwrap()))
                        .collect();
                    out.push(Value::Record(rec));
                }
                Ok(out)
            }
            Type::Array(elem) | Type::Set(elem) => {
                let n = self.count()?;
                let offsets = self.uint32s()?;
                let values = self.vector(elem, Place::Vector)?;
                let elem = (**elem).clone();
                let set = matches!(ty, Type::Set(_));
                Ok(Self::split(&offsets, n, values)?
                    .into_iter()
                    .map(|vals| {
                        if set {
                            Value::Set(elem.clone(), vals)
                        } else {
                            Value::Array(elem.clone(), vals)
                        }
                    })
                    .collect())
            }
            Type::Map(kv) => {
                let n = self.count()?;
                let offsets = self.uint32s()?;
                let keys = self.vector(&kv.0, Place::Vector)?;
                let vals = self.vector(&kv.1, Place::Vector)?;
                if keys.len() != vals.len() {
                    return Err(Error::malformed(
                        "map has different numbers of keys and values",
                    ));
                }
                let entries: Vec<(Value, Value)> = keys.into_iter().zip(vals).collect();
                Ok(Self::split(&offsets, n, entries)?
                    .into_iter()
                    .map(|e| Value::Map(kv.0.clone(), kv.1.clone(), e))
                    .collect())
            }
            Type::Union(members) => {
                let n = self.count()?;
                let tags = self.uint32s()?;
                let mut vecs = Vec::with_capacity(members.len());
                for m in members.iter() {
                    vecs.push(self.vector(m, Place::Vector)?);
                }
                let tags = if members.len() == 2 {
                    tags_from_rle(&tags, n)?
                } else {
                    tags
                };
                if tags.len() != n {
                    return Err(Error::malformed("union has the wrong number of tags"));
                }
                Ok(interleave(&tags, vecs)?
                    .into_iter()
                    .map(|v| Value::Union(ty.clone(), Box::new(v)))
                    .collect())
            }
            Type::Enum(symbols) => {
                let n = self.count()?;
                let vals = intcomp::uint64s(&self.segment()?)?;
                if vals.len() != n {
                    return Err(Error::malformed(
                        "enum vector has the wrong number of values",
                    ));
                }
                vals.into_iter()
                    .map(|k| {
                        if k >= symbols.len() as u64 {
                            return Err(Error::malformed("enum index out of range"));
                        }
                        Ok(Value::Enum(Arc::clone(symbols), k as u32))
                    })
                    .collect()
            }
            Type::Error(inner) => Ok(self
                .vector(inner, Place::Vector)?
                .into_iter()
                .map(|v| Value::Error(Box::new(v)))
                .collect()),
            Type::Named(named) => Ok(self
                .vector(named.inner(), Place::Vector)?
                .into_iter()
                .map(|v| Value::Named(named.clone(), Box::new(v)))
                .collect()),
            Type::Fusion(sup) => {
                let values = self.vector(sup, Place::Vector)?;
                let subtypes = self.uint32s()?;
                if subtypes.len() != values.len() {
                    return Err(Error::malformed(
                        "fusion vector has the wrong number of subtypes",
                    ));
                }
                values
                    .into_iter()
                    .zip(subtypes)
                    .map(|(v, sub)| {
                        let sub = self.tvtypes.lookup(sub)?;
                        let v = if **sup == Type::ALL {
                            match v {
                                Value::Bytes(body) => Value::decode(&sub, &body)?,
                                _ => return Err(Error::malformed("bad fusion(all) vector")),
                            }
                        } else {
                            v
                        };
                        Ok(Value::Fusion((**sup).clone(), Box::new(v), sub))
                    })
                    .collect()
            }
            Type::Option(inner) => {
                let n = self.count()?;
                let tags = self.uint32s()?;
                let inner_ty = (**inner).clone();
                if self.loc.buf.first() == Some(&KIND_NONE) {
                    // A none vector: all the values are absent.
                    self.loc.byte()?;
                    if self.count()? != n {
                        return Err(Error::malformed("none vector has the wrong length"));
                    }
                    return Ok(vec![Value::Option(inner_ty, None); n]);
                }
                let values = self.vector(inner, Place::Option)?;
                if values.len() == n {
                    return Ok(values
                        .into_iter()
                        .map(|v| Value::Option(inner_ty.clone(), Some(Box::new(v))))
                        .collect());
                }
                if tags.len() != n {
                    return Err(Error::malformed("option has the wrong number of tags"));
                }
                let mut it = values.into_iter();
                let out = tags
                    .into_iter()
                    .map(|t| match t {
                        0 => it
                            .next()
                            .map(|v| Value::Option(inner_ty.clone(), Some(Box::new(v))))
                            .ok_or_else(|| Error::malformed("option has too few values")),
                        1 => Ok(Value::Option(inner_ty.clone(), None)),
                        _ => Err(Error::malformed("bad option tag")),
                    })
                    .collect::<Result<Vec<_>>>()?;
                if it.next().is_some() {
                    return Err(Error::malformed("option has too many values"));
                }
                Ok(out)
            }
            _ => Err(Error::malformed(format!(
                "type {ty} has no vector encoding"
            ))),
        }
    }
}

/// Returns the `n` tags of a union of two types encoded as the lengths of
/// alternating runs of tag 1 and tag 0, starting with tag 1, where tags past
/// the runs are 0.
fn tags_from_rle(runs: &[u32], n: usize) -> Result<Vec<u32>> {
    let mut tags = vec![0u32; n];
    let mut off = 0usize;
    for (i, &run) in runs.iter().enumerate() {
        let end = off
            .checked_add(run as usize)
            .filter(|&e| e <= n)
            .ok_or_else(|| Error::malformed("bad union runs"))?;
        if i % 2 == 0 {
            tags[off..end].fill(1);
        }
        off = end;
    }
    Ok(tags)
}
