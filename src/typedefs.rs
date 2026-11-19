//! BSUP typedefs tables, which define the complex types of a frame.

use std::collections::HashMap;
use std::sync::Arc;

use crate::error::{Error, Result};
use crate::types::{check_type_name, Field, NamedType, Primitive, Type, FIRST_COMPLEX_ID};
use crate::varint::{put_name, put_uvarint, Cursor};

pub(crate) const TYPEDEF_RECORD: u8 = 0;
pub(crate) const TYPEDEF_ARRAY: u8 = 1;
pub(crate) const TYPEDEF_SET: u8 = 2;
pub(crate) const TYPEDEF_MAP: u8 = 3;
pub(crate) const TYPEDEF_UNION: u8 = 4;
pub(crate) const TYPEDEF_ENUM: u8 = 5;
pub(crate) const TYPEDEF_ERROR: u8 = 6;
pub(crate) const TYPEDEF_NAMED: u8 = 7;
pub(crate) const TYPEDEF_FUSION: u8 = 8;
pub(crate) const TYPEDEF_OPTION: u8 = 9;
pub(crate) const TYPEDEF_DECIMAL: u8 = 10;

/// Bounds the nesting of types so malformed input can't exhaust the stack.
pub(crate) const MAX_TYPE_DEPTH: usize = 2000;

/// Encodes types into a typedefs table, assigning each complex type an ID.
#[derive(Default)]
pub(crate) struct TypeDefsEncoder {
    bytes: Vec<u8>,
    offsets: Vec<usize>,
    ids: HashMap<Type, u32>,
    names: HashMap<String, (u32, Type)>,
}

impl TypeDefsEncoder {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn reset(&mut self) {
        self.bytes.clear();
        self.offsets.clear();
        self.ids.clear();
        self.names.clear();
    }

    /// Returns the type ID of `ty`, entering it and the types it refers to
    /// in the table if needed.  It fails if a name is bound to two different
    /// types.
    pub(crate) fn lookup(&mut self, ty: &Type) -> Result<u32> {
        if let Type::Primitive(p) = ty {
            return Ok(p.id());
        }
        if let Some(&id) = self.ids.get(ty) {
            return Ok(id);
        }
        let id = match ty {
            Type::Primitive(_) => unreachable!(),
            Type::Record(fields) => {
                let mut ids = Vec::with_capacity(fields.len());
                for f in fields.iter() {
                    ids.push(self.lookup(&f.ty)?);
                }
                let at = self.begin(TYPEDEF_RECORD);
                put_uvarint(&mut self.bytes, fields.len() as u64);
                for (f, id) in fields.iter().zip(ids) {
                    put_name(&mut self.bytes, &f.name);
                    put_uvarint(&mut self.bytes, u64::from(id));
                }
                at
            }
            Type::Array(t) => self.wrapped(TYPEDEF_ARRAY, t)?,
            Type::Set(t) => self.wrapped(TYPEDEF_SET, t)?,
            Type::Error(t) => self.wrapped(TYPEDEF_ERROR, t)?,
            Type::Fusion(t) => self.wrapped(TYPEDEF_FUSION, t)?,
            Type::Option(t) => self.wrapped(TYPEDEF_OPTION, t)?,
            Type::Map(kv) => {
                let k = self.lookup(&kv.0)?;
                let v = self.lookup(&kv.1)?;
                let at = self.begin(TYPEDEF_MAP);
                put_uvarint(&mut self.bytes, u64::from(k));
                put_uvarint(&mut self.bytes, u64::from(v));
                at
            }
            Type::Union(types) => {
                let mut ids = Vec::with_capacity(types.len());
                for t in types.iter() {
                    ids.push(self.lookup(t)?);
                }
                let at = self.begin(TYPEDEF_UNION);
                put_uvarint(&mut self.bytes, ids.len() as u64);
                for id in ids {
                    put_uvarint(&mut self.bytes, u64::from(id));
                }
                at
            }
            Type::Enum(symbols) => {
                let at = self.begin(TYPEDEF_ENUM);
                put_uvarint(&mut self.bytes, symbols.len() as u64);
                for s in symbols.iter() {
                    put_name(&mut self.bytes, s);
                }
                at
            }
            Type::Decimal(p, s) => {
                let at = self.begin(TYPEDEF_DECIMAL);
                put_uvarint(&mut self.bytes, u64::from(*p));
                put_uvarint(&mut self.bytes, u64::from(*s));
                at
            }
            Type::Named(named) => return self.named(ty, named),
        };
        self.ids.insert(ty.clone(), id);
        Ok(id)
    }

    fn named(&mut self, ty: &Type, named: &Arc<NamedType>) -> Result<u32> {
        if let Some((id, bound)) = self.names.get(named.name()) {
            if bound != ty {
                return Err(Error::Redefined(named.name().to_string()));
            }
            return Ok(*id);
        }
        // Declare the name before encoding its underlying type, which may
        // refer to it, and patch in the underlying type's ID afterward.
        let id = self.begin(TYPEDEF_NAMED);
        put_name(&mut self.bytes, named.name());
        let patch = self.bytes.len();
        self.bytes.extend_from_slice(&[0; 4]);
        self.names
            .insert(named.name().to_string(), (id, ty.clone()));
        self.ids.insert(ty.clone(), id);
        let inner = self.lookup(named.inner())?;
        self.bytes[patch..patch + 4].copy_from_slice(&inner.to_be_bytes());
        Ok(id)
    }

    fn wrapped(&mut self, code: u8, inner: &Type) -> Result<u32> {
        let inner = self.lookup(inner)?;
        let at = self.begin(code);
        put_uvarint(&mut self.bytes, u64::from(inner));
        Ok(at)
    }

    fn begin(&mut self, code: u8) -> u32 {
        let id = FIRST_COMPLEX_ID + self.offsets.len() as u32;
        self.offsets.push(self.bytes.len());
        self.bytes.push(code);
        id
    }

    /// Returns the encoding of the table.
    pub(crate) fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        if self.offsets.is_empty() {
            return out;
        }
        put_uvarint(&mut out, self.offsets.len() as u64);
        for &off in &self.offsets {
            put_uvarint(&mut out, off as u64);
        }
        out.extend_from_slice(&self.bytes);
        out
    }
}

/// A decoded typedefs table.
pub(crate) struct TypeDefs {
    types: Vec<Type>,
}

impl TypeDefs {
    /// Decodes a typedefs table.
    pub(crate) fn decode(buf: &[u8]) -> Result<TypeDefs> {
        if buf.is_empty() {
            return Ok(TypeDefs { types: Vec::new() });
        }
        let mut c = Cursor::new(buf);
        let n = c.uvarint()?;
        if n > buf.len() as u64 {
            return Err(Error::malformed("bad number of typedefs"));
        }
        let n = n as usize;
        let mut offsets = Vec::with_capacity(n);
        for _ in 0..n {
            offsets.push(c.uvarint()?);
        }
        let defs = c.buf;
        let mut raw = Vec::with_capacity(n);
        for (i, &off) in offsets.iter().enumerate() {
            let end = if i + 1 < n {
                offsets[i + 1]
            } else {
                defs.len() as u64
            };
            if off > end || end > defs.len() as u64 || (i == 0 && off != 0) {
                return Err(Error::malformed("bad typedef offsets"));
            }
            raw.push(&defs[off as usize..end as usize]);
        }
        let mut dec = Decoder {
            raw,
            types: vec![None; n],
            chain: Vec::new(),
            depth: 0,
        };
        let mut types = Vec::with_capacity(n);
        for i in 0..n {
            types.push(dec.resolve(FIRST_COMPLEX_ID + i as u32)?);
        }
        for t in &types {
            if let Type::Named(named) = t {
                if !named.is_bound() {
                    return Err(Error::malformed("unbound named type"));
                }
            }
        }
        Ok(TypeDefs { types })
    }

    /// Returns the type with ID `id`.
    pub(crate) fn lookup(&self, id: u32) -> Result<Type> {
        if id < FIRST_COMPLEX_ID {
            return Primitive::from_id(id)
                .map(Type::Primitive)
                .ok_or_else(|| Error::malformed(format!("bad type ID {id}")));
        }
        self.types
            .get((id - FIRST_COMPLEX_ID) as usize)
            .cloned()
            .ok_or_else(|| Error::malformed(format!("bad type ID {id}")))
    }
}

struct Decoder<'a> {
    raw: Vec<&'a [u8]>,
    types: Vec<Option<Type>>,
    /// chain holds the named types whose underlying types are being resolved
    /// with no other type between them, which detects a named type that
    /// refers to itself through named types only.
    chain: Vec<usize>,
    depth: usize,
}

impl Decoder<'_> {
    fn resolve(&mut self, id: u32) -> Result<Type> {
        if id < FIRST_COMPLEX_ID {
            return Primitive::from_id(id)
                .map(Type::Primitive)
                .ok_or_else(|| Error::malformed(format!("bad type ID {id}")));
        }
        let k = (id - FIRST_COMPLEX_ID) as usize;
        if k >= self.raw.len() {
            return Err(Error::malformed(format!("bad type ID {id}")));
        }
        if let Some(t) = &self.types[k] {
            if self.chain.contains(&k) {
                return Err(Error::malformed(
                    "named type refers to itself through named types only",
                ));
            }
            return Ok(t.clone());
        }
        self.depth += 1;
        if self.depth > MAX_TYPE_DEPTH {
            return Err(Error::malformed("types nested too deeply"));
        }
        let t = self.resolve_raw(id, k);
        self.depth -= 1;
        let t = t?;
        self.types[k] = Some(t.clone());
        Ok(t)
    }

    /// Resolves a reference from typedef `parent` to `child`, which must
    /// precede it unless the parent is a named type.
    fn child(&mut self, parent: u32, child: u32) -> Result<Type> {
        if child >= parent {
            return Err(Error::malformed(format!(
                "typedef {parent} refers to typedef {child}"
            )));
        }
        self.resolve(child)
    }

    fn resolve_raw(&mut self, id: u32, k: usize) -> Result<Type> {
        let mut c = Cursor::new(self.raw[k]);
        let code = c.byte()?;
        let chain = if code == TYPEDEF_NAMED {
            Vec::new()
        } else {
            std::mem::take(&mut self.chain)
        };
        let t = self.resolve_code(id, k, code, &mut c);
        if code != TYPEDEF_NAMED {
            self.chain = chain;
        }
        let t = t?;
        if !c.is_empty() {
            return Err(Error::malformed("extra bytes in typedef"));
        }
        Ok(t)
    }

    fn resolve_code(&mut self, id: u32, k: usize, code: u8, c: &mut Cursor) -> Result<Type> {
        let t = match code {
            TYPEDEF_RECORD => {
                let n = c.len()?;
                let mut fields = Vec::with_capacity(n);
                for _ in 0..n {
                    let name = c.name()?;
                    let child = c.uvarint_u32()?;
                    fields.push(Field::new(name, self.child(id, child)?));
                }
                Type::record(fields).map_err(|e| Error::malformed(e.to_string()))?
            }
            TYPEDEF_ARRAY => Type::array(self.wrapped(id, c)?),
            TYPEDEF_SET => Type::set(self.wrapped(id, c)?),
            TYPEDEF_ERROR => Type::error(self.wrapped(id, c)?),
            TYPEDEF_FUSION => Type::fusion(self.wrapped(id, c)?),
            TYPEDEF_OPTION => {
                let inner = self.wrapped(id, c)?;
                Type::option(inner).map_err(|e| Error::malformed(e.to_string()))?
            }
            TYPEDEF_MAP => {
                let key = c.uvarint_u32()?;
                let val = c.uvarint_u32()?;
                Type::map(self.child(id, key)?, self.child(id, val)?)
            }
            TYPEDEF_UNION => {
                let n = c.len()?;
                let mut types = Vec::with_capacity(n);
                for _ in 0..n {
                    let child = c.uvarint_u32()?;
                    let t = self.child(id, child)?;
                    if matches!(t, Type::Union(_)) {
                        return Err(Error::malformed("union member is a union"));
                    }
                    types.push(t);
                }
                if types.is_empty() {
                    return Err(Error::malformed("union of no types"));
                }
                // Keep the members in the order written since the order
                // defines the tags of the union's values.
                Type::Union(types.into())
            }
            TYPEDEF_ENUM => {
                let n = c.len()?;
                let mut symbols = Vec::with_capacity(n);
                for _ in 0..n {
                    symbols.push(c.name()?);
                }
                Type::enumeration(symbols)
            }
            TYPEDEF_DECIMAL => {
                let p = c.uvarint_u32()?;
                let s = c.uvarint_u32()?;
                Type::Decimal(p, s)
            }
            TYPEDEF_NAMED => {
                let name = c.name()?;
                check_type_name(&name).map_err(|e| Error::malformed(e.to_string()))?;
                let b = c.bytes(4)?;
                let inner = u32::from_be_bytes([b[0], b[1], b[2], b[3]]);
                let named = NamedType::declare(name);
                let t = Type::Named(named.clone());
                // Enter the named type before resolving its underlying
                // type, which may refer to it.
                self.types[k] = Some(t.clone());
                self.chain.push(k);
                let result = self.resolve(inner);
                self.chain.pop();
                let inner = match result {
                    Ok(inner) => inner,
                    Err(err) => {
                        self.types[k] = None;
                        return Err(err);
                    }
                };
                named.bind(inner);
                t
            }
            _ => return Err(Error::malformed(format!("unknown typedef code {code}"))),
        };
        Ok(t)
    }

    fn wrapped(&mut self, id: u32, c: &mut Cursor) -> Result<Type> {
        let child = c.uvarint_u32()?;
        self.child(id, child)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let rec = Type::record(vec![
            Field::new("a", Type::INT64),
            Field::new(
                "b",
                Type::array(Type::union([Type::STRING, Type::NULL]).unwrap()),
            ),
        ])
        .unwrap();
        let named = Type::named("Thing", rec.clone()).unwrap();
        let types = vec![
            rec,
            named.clone(),
            Type::map(Type::STRING, named),
            Type::enumeration(vec!["a".into(), "b".into()]),
            Type::option(Type::INT64).unwrap(),
            Type::fusion(Type::ALL),
            Type::error(Type::STRING),
            Type::set(Type::IP),
            Type::Decimal(10, 2),
        ];
        let mut enc = TypeDefsEncoder::new();
        let ids: Vec<u32> = types.iter().map(|t| enc.lookup(t).unwrap()).collect();
        assert_eq!(enc.lookup(&types[0]).unwrap(), ids[0]);
        let defs = TypeDefs::decode(&enc.encode()).unwrap();
        for (t, id) in types.iter().zip(ids) {
            assert_eq!(&defs.lookup(id).unwrap(), t);
        }
    }

    #[test]
    fn redefinition() {
        let mut enc = TypeDefsEncoder::new();
        enc.lookup(&Type::named("T", Type::INT64).unwrap()).unwrap();
        assert!(matches!(
            enc.lookup(&Type::named("T", Type::STRING).unwrap()),
            Err(Error::Redefined(_))
        ));
    }

    #[test]
    fn recursive_named() {
        // list={next:list|null} as Go would write it: the named typedef
        // first, referring forward to its underlying record type.
        let mut buf = Vec::new();
        put_uvarint(&mut buf, 3);
        let mut defs = Vec::new();
        let mut offs = Vec::new();
        offs.push(defs.len());
        defs.push(TYPEDEF_NAMED);
        put_name(&mut defs, "list");
        defs.extend_from_slice(&50u32.to_be_bytes());
        offs.push(defs.len());
        defs.push(TYPEDEF_UNION);
        put_uvarint(&mut defs, 2);
        put_uvarint(&mut defs, 48);
        put_uvarint(&mut defs, Primitive::Null.id() as u64);
        offs.push(defs.len());
        defs.push(TYPEDEF_RECORD);
        put_uvarint(&mut defs, 1);
        put_name(&mut defs, "next");
        put_uvarint(&mut defs, 49);
        for o in offs {
            put_uvarint(&mut buf, o as u64);
        }
        buf.extend_from_slice(&defs);
        let defs = TypeDefs::decode(&buf).unwrap();
        let list = defs.lookup(48).unwrap();
        assert_eq!(format!("{list:?}"), "list={next:list|null}");
        // A named type can't refer to itself through named types alone.
        let mut buf = Vec::new();
        put_uvarint(&mut buf, 1);
        put_uvarint(&mut buf, 0);
        buf.push(TYPEDEF_NAMED);
        put_name(&mut buf, "x");
        buf.extend_from_slice(&48u32.to_be_bytes());
        assert!(TypeDefs::decode(&buf).is_err());
    }
}
