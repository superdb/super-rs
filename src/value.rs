//! Super-structured values.

use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::Arc;

use crate::error::{Error, Result};
use crate::types::{Field, NamedType, Primitive, Type};
use crate::typevalue::{decode_type_value, encode_type_value};
use crate::varint::{decode_int, decode_uint, put_body, put_int, put_uint, Cursor};

/// A super-structured value.
///
/// A value knows its type (see [`Value::typ`]).  Where the type of a value
/// can't be derived from its parts (e.g., the element type of an empty array
/// or the union type of a union value), the value holds that type.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    #[allow(missing_docs)]
    Uint8(u8),
    #[allow(missing_docs)]
    Uint16(u16),
    #[allow(missing_docs)]
    Uint32(u32),
    #[allow(missing_docs)]
    Uint64(u64),
    #[allow(missing_docs)]
    Int8(i8),
    #[allow(missing_docs)]
    Int16(i16),
    #[allow(missing_docs)]
    Int32(i32),
    #[allow(missing_docs)]
    Int64(i64),
    /// A duration in nanoseconds.
    Duration(i64),
    /// A timestamp in nanoseconds since the Unix epoch.
    Timestamp(i64),
    /// A float16 value, held as an `f32`.
    Float16(f32),
    #[allow(missing_docs)]
    Float32(f32),
    #[allow(missing_docs)]
    Float64(f64),
    #[allow(missing_docs)]
    Bool(bool),
    #[allow(missing_docs)]
    Bytes(Vec<u8>),
    #[allow(missing_docs)]
    String(String),
    #[allow(missing_docs)]
    Ip(IpAddr),
    /// A network: an address, masked by the prefix, and the prefix length.
    Net(IpAddr, u8),
    #[allow(missing_docs)]
    Uuid([u8; 16]),
    /// A type value.
    Type(Type),
    /// The null value.
    Null,
    /// The missing value (e.g., a field absent from a value of a fused type).
    Missing,
    /// The none value.
    None,
    /// A record with its fields in order.
    Record(Vec<(String, Value)>),
    /// An array with its element type.
    Array(Type, Vec<Value>),
    /// A set with its element type.
    Set(Type, Vec<Value>),
    /// A map with its key type, value type, and entries.
    Map(Type, Type, Vec<(Value, Value)>),
    /// A union value with the union type, whose members include the type of
    /// the value.
    Union(Type, Box<Value>),
    /// An enum value with the enum's symbols and the index of its symbol.
    Enum(Arc<[String]>, u32),
    /// An error value.
    Error(Box<Value>),
    /// A value of a named type, whose underlying type is the type of the
    /// value.
    Named(Arc<NamedType>, Box<Value>),
    /// A value of an option type with the option's inner type.
    Option(Type, Option<Box<Value>>),
    /// A value of a fusion type: the supertype, the value as a value of the
    /// supertype (or of its actual type if the supertype is `all`), and the
    /// actual type of the value.
    Fusion(Type, Box<Value>, Type),
    /// A value of a type whose values are reserved by the BSUP specification
    /// (e.g., `int128` or `decimal`), with its body.
    Reserved(Type, Vec<u8>),
}

impl Value {
    /// Returns a record value.
    pub fn record<I, S>(fields: I) -> Value
    where
        I: IntoIterator<Item = (S, Value)>,
        S: Into<String>,
    {
        Value::Record(fields.into_iter().map(|(k, v)| (k.into(), v)).collect())
    }

    /// Returns the type of the value.
    pub fn typ(&self) -> Type {
        match self {
            Value::Uint8(_) => Type::UINT8,
            Value::Uint16(_) => Type::UINT16,
            Value::Uint32(_) => Type::UINT32,
            Value::Uint64(_) => Type::UINT64,
            Value::Int8(_) => Type::INT8,
            Value::Int16(_) => Type::INT16,
            Value::Int32(_) => Type::INT32,
            Value::Int64(_) => Type::INT64,
            Value::Duration(_) => Type::DURATION,
            Value::Timestamp(_) => Type::TIMESTAMP,
            Value::Float16(_) => Type::FLOAT16,
            Value::Float32(_) => Type::FLOAT32,
            Value::Float64(_) => Type::FLOAT64,
            Value::Bool(_) => Type::BOOL,
            Value::Bytes(_) => Type::BYTES,
            Value::String(_) => Type::STRING,
            Value::Ip(_) => Type::IP,
            Value::Net(..) => Type::NET,
            Value::Uuid(_) => Type::UUID,
            Value::Type(_) => Type::TYPE,
            Value::Null => Type::NULL,
            Value::Missing => Type::MISSING,
            Value::None => Type::NONE,
            Value::Record(fields) => Type::Record(
                fields
                    .iter()
                    .map(|(name, v)| Field::new(name.clone(), v.typ()))
                    .collect(),
            ),
            Value::Array(elem, _) => Type::array(elem.clone()),
            Value::Set(elem, _) => Type::set(elem.clone()),
            Value::Map(k, v, _) => Type::map(k.clone(), v.clone()),
            Value::Union(t, _) => t.clone(),
            Value::Enum(symbols, _) => Type::Enum(symbols.clone()),
            Value::Error(v) => Type::error(v.typ()),
            Value::Named(n, _) => Type::Named(n.clone()),
            Value::Option(t, _) => Type::Option(Arc::new(t.clone())),
            Value::Fusion(t, ..) => Type::fusion(t.clone()),
            Value::Reserved(t, _) => t.clone(),
        }
    }

    /// Returns true if the value is null, missing, none, or an option
    /// without a value (looking through union, named, and fusion values).
    pub fn is_absent(&self) -> bool {
        matches!(
            self.under(),
            Value::Null | Value::Missing | Value::None | Value::Option(_, None)
        )
    }

    /// Returns the value inside union, named, option, and fusion values.
    pub fn under(&self) -> &Value {
        match self {
            Value::Union(_, v)
            | Value::Named(_, v)
            | Value::Option(_, Some(v))
            | Value::Fusion(_, v, _) => v.under(),
            v => v,
        }
    }

    /// Returns the value with each value of a fusion type replaced by the
    /// value of its subtype (the value's actual type), as SuperDB's defuse
    /// does.
    pub fn defuse(&self) -> Value {
        match self {
            Value::Fusion(_, v, sub) => defuse_to(v, sub),
            Value::Record(fields) => Value::Record(
                fields
                    .iter()
                    .map(|(n, v)| (n.clone(), v.defuse()))
                    .collect(),
            ),
            Value::Array(t, vals) => {
                let vals: Vec<Value> = vals.iter().map(Value::defuse).collect();
                Value::Array(defused_elem(t, &vals), vals)
            }
            Value::Set(t, vals) => {
                let vals: Vec<Value> = vals.iter().map(Value::defuse).collect();
                Value::Set(defused_elem(t, &vals), vals)
            }
            Value::Map(k, v, entries) => {
                let entries: Vec<(Value, Value)> = entries
                    .iter()
                    .map(|(k, v)| (k.defuse(), v.defuse()))
                    .collect();
                let keys: Vec<Value> = entries.iter().map(|(k, _)| k.clone()).collect();
                let vals: Vec<Value> = entries.iter().map(|(_, v)| v.clone()).collect();
                Value::Map(defused_elem(k, &keys), defused_elem(v, &vals), entries)
            }
            Value::Union(t, v) => {
                let v = v.defuse();
                let vt = v.typ();
                if t.union_tag(&vt).is_some() {
                    Value::Union(t.clone(), Box::new(v))
                } else {
                    v
                }
            }
            Value::Error(v) => Value::Error(Box::new(v.defuse())),
            Value::Option(t, Some(v)) => {
                let v = v.defuse();
                Value::Option(
                    if has_fusion(t) { v.typ() } else { t.clone() },
                    Some(Box::new(v)),
                )
            }
            v => v.clone(),
        }
    }

    /// Returns the field `name` of a record value.
    pub fn field(&self, name: &str) -> Option<&Value> {
        match self.under() {
            Value::Record(fields) => fields.iter().find(|(n, _)| n == name).map(|(_, v)| v),
            _ => None,
        }
    }

    /// Appends the body of the value to `buf`.
    pub fn encode_body(&self, buf: &mut Vec<u8>) -> Result<()> {
        match self {
            Value::Uint8(v) => put_uint(buf, u64::from(*v)),
            Value::Uint16(v) => put_uint(buf, u64::from(*v)),
            Value::Uint32(v) => put_uint(buf, u64::from(*v)),
            Value::Uint64(v) => put_uint(buf, *v),
            Value::Int8(v) => put_int(buf, i64::from(*v)),
            Value::Int16(v) => put_int(buf, i64::from(*v)),
            Value::Int32(v) => put_int(buf, i64::from(*v)),
            Value::Int64(v) | Value::Duration(v) | Value::Timestamp(v) => put_int(buf, *v),
            Value::Float16(v) => buf.extend_from_slice(&f32_to_f16(*v).to_le_bytes()),
            Value::Float32(v) => buf.extend_from_slice(&v.to_le_bytes()),
            Value::Float64(v) => buf.extend_from_slice(&v.to_le_bytes()),
            Value::Bool(v) => buf.push(u8::from(*v)),
            Value::Bytes(b) => buf.extend_from_slice(b),
            Value::String(s) => buf.extend_from_slice(s.as_bytes()),
            Value::Ip(ip) => put_ip(buf, ip),
            Value::Net(ip, prefix) => {
                let (addr, mask) = net_bytes(ip, *prefix)?;
                buf.extend_from_slice(&addr);
                buf.extend_from_slice(&mask);
            }
            Value::Uuid(u) => buf.extend_from_slice(u),
            Value::Type(t) => buf.extend_from_slice(&encode_type_value(t)?),
            Value::Null | Value::Missing | Value::None => {}
            Value::Record(fields) => {
                for (_, v) in fields {
                    put_value(buf, v)?;
                }
            }
            Value::Array(_, vals) => {
                for v in vals {
                    put_value(buf, v)?;
                }
            }
            Value::Set(_, vals) => {
                let mut elems = Vec::with_capacity(vals.len());
                for v in vals {
                    let mut b = Vec::new();
                    put_value(&mut b, v)?;
                    elems.push(b);
                }
                elems.sort();
                elems.dedup();
                for e in elems {
                    buf.extend_from_slice(&e);
                }
            }
            Value::Map(_, _, entries) => {
                let mut kvs = Vec::with_capacity(entries.len());
                for (k, v) in entries {
                    let mut kb = Vec::new();
                    put_value(&mut kb, k)?;
                    let mut vb = Vec::new();
                    put_value(&mut vb, v)?;
                    kvs.push((kb, vb));
                }
                // Sort stably by key and keep the first entry of each key.
                kvs.sort_by(|a, b| a.0.cmp(&b.0));
                kvs.dedup_by(|a, b| a.0 == b.0);
                for (k, v) in kvs {
                    buf.extend_from_slice(&k);
                    buf.extend_from_slice(&v);
                }
            }
            Value::Union(t, v) => {
                let tag = t.union_tag(&v.typ()).ok_or_else(|| {
                    Error::msg(format!("type {} is not a member of union {t}", v.typ()))
                })?;
                let mut b = Vec::new();
                put_uint(&mut b, tag as u64);
                put_body(buf, &b);
                put_value(buf, v)?;
            }
            Value::Enum(symbols, k) => {
                if *k as usize >= symbols.len() {
                    return Err(Error::msg("enum index out of range"));
                }
                put_uint(buf, u64::from(*k));
            }
            Value::Error(v) | Value::Named(_, v) => v.encode_body(buf)?,
            Value::Option(_, v) => match v {
                Some(v) => {
                    put_body(buf, &[]);
                    put_value(buf, v)?;
                }
                None => put_body(buf, &[1]),
            },
            Value::Fusion(_, v, sub) => {
                put_value(buf, v)?;
                put_body(buf, &encode_type_value(sub)?);
            }
            Value::Reserved(_, b) => buf.extend_from_slice(b),
        }
        Ok(())
    }

    /// Decodes the body of a value of type `ty`.
    pub fn decode(ty: &Type, body: &[u8]) -> Result<Value> {
        decode(ty, body, 0)
    }
}

/// Returns true if `t` has a fusion type outside the body of a named type.
fn has_fusion(t: &Type) -> bool {
    match t {
        Type::Fusion(_) => true,
        Type::Record(fields) => fields.iter().any(|f| has_fusion(&f.ty)),
        Type::Array(x) | Type::Set(x) | Type::Error(x) | Type::Option(x) => has_fusion(x),
        Type::Map(kv) => has_fusion(&kv.0) || has_fusion(&kv.1),
        Type::Union(types) => types.iter().any(has_fusion),
        _ => false,
    }
}

/// Returns the element type of a container whose element type was `t` and
/// whose elements, now defused, are `vals`.
fn defused_elem(t: &Type, vals: &[Value]) -> Type {
    if !has_fusion(t) {
        return t.clone();
    }
    if vals.is_empty() {
        return Type::NONE;
    }
    Type::union(vals.iter().map(|v| match v {
        Value::Union(_, inner) => inner.typ(),
        v => v.typ(),
    }))
    .unwrap_or(Type::NONE)
}

/// Converts `v`, a value of a fusion's supertype, to a value of its subtype
/// `sub`.
fn defuse_to(v: &Value, sub: &Type) -> Value {
    if v.typ() == *sub {
        return v.defuse();
    }
    match (v, sub) {
        (Value::Fusion(_, inner, s), _) => defuse_to(&defuse_to(inner, s), sub),
        (Value::Union(_, inner), _) if !matches!(sub, Type::Union(_)) => defuse_to(inner, sub),
        (_, Type::Union(members)) => {
            let inner = match v {
                Value::Union(_, inner) => inner,
                v => v,
            };
            let vt = inner.typ();
            if members.contains(&vt) {
                return Value::Union(sub.clone(), Box::new(inner.defuse()));
            }
            match members
                .iter()
                .find(|m| m.kind() == vt.kind() && !m.is_named())
            {
                Some(m) => Value::Union(sub.clone(), Box::new(defuse_to(inner, m))),
                None => inner.defuse(),
            }
        }
        (Value::Record(fields), Type::Record(sf)) => Value::Record(
            sf.iter()
                .filter_map(|f| {
                    fields
                        .iter()
                        .find(|(n, _)| *n == f.name)
                        .map(|(n, v)| (n.clone(), defuse_to(v, &f.ty)))
                })
                .collect(),
        ),
        (Value::Array(_, vals), Type::Array(e)) => Value::Array(
            (**e).clone(),
            vals.iter().map(|v| defuse_to(v, e)).collect(),
        ),
        (Value::Set(_, vals), Type::Set(e)) => Value::Set(
            (**e).clone(),
            vals.iter().map(|v| defuse_to(v, e)).collect(),
        ),
        (Value::Map(_, _, entries), Type::Map(kv)) => Value::Map(
            kv.0.clone(),
            kv.1.clone(),
            entries
                .iter()
                .map(|(k, v)| (defuse_to(k, &kv.0), defuse_to(v, &kv.1)))
                .collect(),
        ),
        (Value::Option(_, Some(x)), Type::Option(t)) => {
            Value::Option((**t).clone(), Some(Box::new(defuse_to(x, t))))
        }
        (Value::Option(_, None), Type::Option(t)) => Value::Option((**t).clone(), None),
        (Value::Option(_, Some(x)), _) => defuse_to(x, sub),
        (Value::Error(x), Type::Error(t)) => Value::Error(Box::new(defuse_to(x, t))),
        (Value::Enum(symbols, k), Type::Enum(sub_symbols)) => {
            let name = &symbols[*k as usize];
            match sub_symbols.iter().position(|s| s == name) {
                Some(k) => Value::Enum(sub_symbols.clone(), k as u32),
                None => v.clone(),
            }
        }
        _ => v.defuse(),
    }
}

fn put_value(buf: &mut Vec<u8>, v: &Value) -> Result<()> {
    let mut b = Vec::new();
    v.encode_body(&mut b)?;
    put_body(buf, &b);
    Ok(())
}

fn put_ip(buf: &mut Vec<u8>, ip: &IpAddr) {
    match ip {
        IpAddr::V4(a) => buf.extend_from_slice(&a.octets()),
        IpAddr::V6(a) => buf.extend_from_slice(&a.octets()),
    }
}

fn net_bytes(ip: &IpAddr, prefix: u8) -> Result<(Vec<u8>, Vec<u8>)> {
    let mut addr = Vec::new();
    put_ip(&mut addr, ip);
    let bits = addr.len() * 8;
    if usize::from(prefix) > bits {
        return Err(Error::msg(format!("bad network prefix length {prefix}")));
    }
    let mut mask = vec![0u8; addr.len()];
    for i in 0..usize::from(prefix) {
        mask[i / 8] |= 0x80 >> (i % 8);
    }
    for (a, m) in addr.iter_mut().zip(&mask) {
        *a &= m;
    }
    Ok((addr, mask))
}

const MAX_VALUE_DEPTH: usize = 2000;

fn decode(ty: &Type, body: &[u8], depth: usize) -> Result<Value> {
    if depth > MAX_VALUE_DEPTH {
        return Err(Error::malformed("value nested too deeply"));
    }
    let depth = depth + 1;
    Ok(match ty {
        Type::Primitive(p) => decode_primitive(*p, body)?,
        Type::Record(fields) => {
            let mut c = Cursor::new(body);
            let mut out = Vec::with_capacity(fields.len());
            for f in fields.iter() {
                out.push((f.name.clone(), decode(&f.ty, c.body()?, depth)?));
            }
            check_end(&c)?;
            Value::Record(out)
        }
        Type::Array(elem) | Type::Set(elem) => {
            let mut c = Cursor::new(body);
            let mut out = Vec::new();
            while !c.is_empty() {
                out.push(decode(elem, c.body()?, depth)?);
            }
            if matches!(ty, Type::Array(_)) {
                Value::Array((**elem).clone(), out)
            } else {
                Value::Set((**elem).clone(), out)
            }
        }
        Type::Map(kv) => {
            let mut c = Cursor::new(body);
            let mut out = Vec::new();
            while !c.is_empty() {
                let k = decode(&kv.0, c.body()?, depth)?;
                let v = decode(&kv.1, c.body()?, depth)?;
                out.push((k, v));
            }
            Value::Map(kv.0.clone(), kv.1.clone(), out)
        }
        Type::Union(types) => {
            let mut c = Cursor::new(body);
            let tag = decode_uint(c.body()?)?;
            let member = types
                .get(tag as usize)
                .ok_or_else(|| Error::malformed(format!("union tag {tag} out of range")))?;
            let v = decode(member, c.body()?, depth)?;
            check_end(&c)?;
            Value::Union(ty.clone(), Box::new(v))
        }
        Type::Enum(symbols) => {
            let k = decode_uint(body)?;
            if k >= symbols.len() as u64 {
                return Err(Error::malformed("enum index out of range"));
            }
            Value::Enum(symbols.clone(), k as u32)
        }
        Type::Error(t) => Value::Error(Box::new(decode(t, body, depth)?)),
        Type::Named(n) => Value::Named(n.clone(), Box::new(decode(n.inner(), body, depth)?)),
        Type::Option(t) => {
            let mut c = Cursor::new(body);
            let tag = decode_uint(c.body()?)?;
            let v = match tag {
                0 => Some(Box::new(decode(t, c.body()?, depth)?)),
                1 => None,
                _ => return Err(Error::malformed("bad option tag")),
            };
            check_end(&c)?;
            Value::Option((**t).clone(), v)
        }
        Type::Fusion(t) => {
            let mut c = Cursor::new(body);
            let vbody = c.body()?;
            let sub = decode_type_value(c.body()?)?;
            check_end(&c)?;
            // A value of fusion(all) is a value of its subtype.
            let as_type = if **t == Type::ALL { &sub } else { &**t };
            let v = decode(as_type, vbody, depth)?;
            Value::Fusion((**t).clone(), Box::new(v), sub)
        }
        Type::Decimal(..) => Value::Reserved(ty.clone(), body.to_vec()),
    })
}

fn check_end(c: &Cursor) -> Result<()> {
    if c.is_empty() {
        Ok(())
    } else {
        Err(Error::malformed("extra bytes in container"))
    }
}

fn decode_primitive(p: Primitive, body: &[u8]) -> Result<Value> {
    use Primitive as P;
    let narrow = |v: u64, max: u64| -> Result<u64> {
        if v > max {
            Err(Error::malformed("integer out of range for its type"))
        } else {
            Ok(v)
        }
    };
    let narrow_int = |v: i64, min: i64, max: i64| -> Result<i64> {
        if v < min || v > max {
            Err(Error::malformed("integer out of range for its type"))
        } else {
            Ok(v)
        }
    };
    let fixed = |n: usize| -> Result<&[u8]> {
        if body.len() == n {
            Ok(body)
        } else {
            Err(Error::malformed(format!(
                "{} body has length {}",
                p.name(),
                body.len()
            )))
        }
    };
    Ok(match p {
        P::Uint8 => Value::Uint8(narrow(decode_uint(body)?, u8::MAX.into())? as u8),
        P::Uint16 => Value::Uint16(narrow(decode_uint(body)?, u16::MAX.into())? as u16),
        P::Uint32 => Value::Uint32(narrow(decode_uint(body)?, u32::MAX.into())? as u32),
        P::Uint64 => Value::Uint64(decode_uint(body)?),
        P::Int8 => {
            Value::Int8(narrow_int(decode_int(body)?, i8::MIN.into(), i8::MAX.into())? as i8)
        }
        P::Int16 => {
            Value::Int16(narrow_int(decode_int(body)?, i16::MIN.into(), i16::MAX.into())? as i16)
        }
        P::Int32 => {
            Value::Int32(narrow_int(decode_int(body)?, i32::MIN.into(), i32::MAX.into())? as i32)
        }
        P::Int64 => Value::Int64(decode_int(body)?),
        P::Duration => Value::Duration(decode_int(body)?),
        P::Timestamp => Value::Timestamp(decode_int(body)?),
        P::Float16 => {
            let b = fixed(2)?;
            Value::Float16(f16_to_f32(u16::from_le_bytes([b[0], b[1]])))
        }
        P::Float32 => {
            let b = fixed(4)?;
            Value::Float32(f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        }
        P::Float64 => {
            let b = fixed(8)?;
            let mut a = [0u8; 8];
            a.copy_from_slice(b);
            Value::Float64(f64::from_le_bytes(a))
        }
        P::Bool => match fixed(1)?[0] {
            0 => Value::Bool(false),
            1 => Value::Bool(true),
            _ => return Err(Error::malformed("bad bool body")),
        },
        P::Bytes => Value::Bytes(body.to_vec()),
        P::String => Value::String(
            String::from_utf8(body.to_vec())
                .map_err(|_| Error::malformed("string is not valid UTF-8"))?,
        ),
        P::Ip => Value::Ip(decode_ip(body)?),
        P::Net => {
            if body.len() != 8 && body.len() != 32 {
                return Err(Error::malformed("bad net body"));
            }
            let (addr, mask) = body.split_at(body.len() / 2);
            let mut prefix = 0u32;
            for &b in mask {
                prefix += b.leading_ones();
                if b != 0xff {
                    break;
                }
            }
            Value::Net(decode_ip(addr)?, prefix as u8)
        }
        P::Uuid => {
            let mut u = [0u8; 16];
            u.copy_from_slice(fixed(16)?);
            Value::Uuid(u)
        }
        P::Type => Value::Type(decode_type_value(body)?),
        P::Null => Value::Null,
        P::Missing => Value::Missing,
        P::None => Value::None,
        P::All => {
            return Err(Error::malformed(
                "value of type all outside of a fusion value",
            ))
        }
        _ => Value::Reserved(Type::Primitive(p), body.to_vec()),
    })
}

fn decode_ip(b: &[u8]) -> Result<IpAddr> {
    match b.len() {
        4 => Ok(IpAddr::V4(Ipv4Addr::new(b[0], b[1], b[2], b[3]))),
        16 => {
            let mut a = [0u8; 16];
            a.copy_from_slice(b);
            Ok(IpAddr::V6(Ipv6Addr::from(a)))
        }
        _ => Err(Error::malformed("bad ip body")),
    }
}

/// Converts an IEEE 754 binary16 value to an `f32`.
pub(crate) fn f16_to_f32(h: u16) -> f32 {
    let sign = u32::from(h >> 15) << 31;
    let exp = u32::from((h >> 10) & 0x1f);
    let frac = u32::from(h & 0x3ff);
    let bits = match (exp, frac) {
        (0, 0) => sign,
        (0, _) => {
            // Subnormal: normalize it.
            let mut e = 127 - 15 + 1;
            let mut f = frac;
            while f & 0x400 == 0 {
                f <<= 1;
                e -= 1;
            }
            sign | (e << 23) | ((f & 0x3ff) << 13)
        }
        (0x1f, 0) => sign | 0x7f80_0000,
        (0x1f, _) => sign | 0x7fc0_0000 | (frac << 13),
        _ => sign | ((exp + 127 - 15) << 23) | (frac << 13),
    };
    f32::from_bits(bits)
}

/// Converts an `f32` to the nearest IEEE 754 binary16 value (rounding half
/// to even).
pub(crate) fn f32_to_f16(v: f32) -> u16 {
    let bits = v.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exp = ((bits >> 23) & 0xff) as i32;
    let frac = bits & 0x7f_ffff;
    if exp == 0xff {
        return sign | 0x7c00 | if frac != 0 { 0x200 } else { 0 };
    }
    let e = exp - 127 + 15;
    if e >= 0x1f {
        return sign | 0x7c00;
    }
    if e <= 0 {
        if e < -10 {
            return sign;
        }
        let f = frac | 0x80_0000;
        let shift = (14 - e) as u32;
        let half = 1u32 << (shift - 1);
        let mut h = f >> shift;
        let rem = f & ((1 << shift) - 1);
        if rem > half || (rem == half && h & 1 == 1) {
            h += 1;
        }
        return sign | h as u16;
    }
    let mut h = ((e as u32) << 10) | (frac >> 13);
    let rem = frac & 0x1fff;
    if rem > 0x1000 || (rem == 0x1000 && h & 1 == 1) {
        h += 1;
    }
    sign | h as u16
}

pub(crate) fn quote_string(s: &mut String, v: &str) {
    s.push('"');
    for c in v.chars() {
        match c {
            '"' => s.push_str("\\\""),
            '\\' => s.push_str("\\\\"),
            '\n' => s.push_str("\\n"),
            '\r' => s.push_str("\\r"),
            '\t' => s.push_str("\\t"),
            '\u{8}' => s.push_str("\\b"),
            '\u{c}' => s.push_str("\\f"),
            c if (c as u32) < 0x20 => s.push_str(&format!("\\u{:04x}", c as u32)),
            c => s.push(c),
        }
    }
    s.push('"');
}

/// Formats a value in SUP syntax (see [`crate::sup`]).
impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&crate::sup::format_value(self))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(v: Value) {
        let mut body = Vec::new();
        v.encode_body(&mut body).unwrap();
        assert_eq!(Value::decode(&v.typ(), &body).unwrap(), v, "{v}");
    }

    #[test]
    fn roundtrips() {
        let u = Type::union([Type::INT64, Type::STRING]).unwrap();
        let point = NamedType::declare("Point");
        point.bind(Type::record(vec![Field::new("x", Type::INT32)]).unwrap());
        for v in [
            Value::Uint8(255),
            Value::Int16(-300),
            Value::Int64(i64::MIN),
            Value::Float16(1.5),
            Value::Float32(-2.25),
            Value::Float64(3.0e100),
            Value::Bool(true),
            Value::Bytes(vec![]),
            Value::String("héllo".into()),
            Value::Ip("10.0.0.1".parse().unwrap()),
            Value::Ip("::1".parse().unwrap()),
            Value::Net("10.1.0.0".parse().unwrap(), 16),
            Value::Uuid([7; 16]),
            Value::Type(u.clone()),
            Value::Null,
            Value::record([
                ("a", Value::Int64(1)),
                ("b", Value::Array(Type::NONE, vec![])),
            ]),
            Value::Array(
                u.clone(),
                vec![Value::Union(u.clone(), Box::new(Value::String("x".into())))],
            ),
            Value::Set(Type::INT64, vec![Value::Int64(1), Value::Int64(2)]),
            Value::Map(
                Type::STRING,
                Type::INT64,
                vec![(Value::String("k".into()), Value::Int64(1))],
            ),
            Value::Enum(vec!["a".to_string(), "b".to_string()].into(), 1),
            Value::Error(Box::new(Value::String("oops".into()))),
            Value::Named(
                point.clone(),
                Box::new(Value::record([("x", Value::Int32(7))])),
            ),
            Value::Option(Type::INT64, Some(Box::new(Value::Int64(5)))),
            Value::Option(Type::INT64, None),
            Value::Fusion(Type::ALL, Box::new(Value::Int64(3)), Type::INT64),
            Value::Fusion(
                Type::union([Type::INT64, Type::STRING]).unwrap(),
                Box::new(Value::Union(u, Box::new(Value::Int64(1)))),
                Type::INT64,
            ),
        ] {
            roundtrip(v);
        }
    }

    #[test]
    fn set_normalized() {
        let v = Value::Set(
            Type::INT64,
            vec![Value::Int64(2), Value::Int64(1), Value::Int64(2)],
        );
        let mut body = Vec::new();
        v.encode_body(&mut body).unwrap();
        assert_eq!(
            Value::decode(&v.typ(), &body).unwrap(),
            Value::Set(Type::INT64, vec![Value::Int64(1), Value::Int64(2)])
        );
    }

    #[test]
    fn float16() {
        for v in [
            0.0f32,
            1.0,
            -2.5,
            65504.0,
            0.000061035156,
            5.9604645e-8,
            f32::INFINITY,
        ] {
            assert_eq!(f16_to_f32(f32_to_f16(v)), v);
        }
        assert!(f16_to_f32(f32_to_f16(f32::NAN)).is_nan());
    }

    #[test]
    fn display() {
        let v = Value::record([
            ("a", Value::Int64(1)),
            ("b", Value::Float64(2.0)),
            ("c", Value::Int32(3)),
            ("d d", Value::String("x\"y".into())),
        ]);
        assert_eq!(v.to_string(), r#"{a:1,b:2.,c:3::int32,"d d":"x\"y"}"#);
    }
}
