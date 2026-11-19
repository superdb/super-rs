//! Type values: the encoding of a type that doesn't depend on a typedefs
//! table (e.g., the body of a value of type `type`).

use std::collections::HashMap;

use crate::error::{Error, Result};
use crate::typedefs::MAX_TYPE_DEPTH;
use crate::types::{check_type_name, Field, NamedType, Primitive, Type};
use crate::varint::{put_name, put_uvarint, Cursor};

const TV_RECORD: u8 = 48;
const TV_ARRAY: u8 = 49;
const TV_SET: u8 = 50;
const TV_MAP: u8 = 51;
const TV_UNION: u8 = 52;
const TV_ENUM: u8 = 53;
const TV_ERROR: u8 = 54;
const TV_NAME_DEF: u8 = 55;
const TV_NAME_REF: u8 = 56;
const TV_FUSION: u8 = 57;
const TV_OPTION: u8 = 58;
const TV_DECIMAL: u8 = 59;

/// Returns the type value of `ty`.
pub fn encode_type_value(ty: &Type) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    append(&mut buf, ty, &mut HashMap::new())?;
    Ok(buf)
}

fn append(buf: &mut Vec<u8>, ty: &Type, defs: &mut HashMap<String, Type>) -> Result<()> {
    match ty {
        Type::Primitive(p) => buf.push(p.id() as u8),
        Type::Named(named) => {
            if let Some(prev) = defs.get(named.name()) {
                if prev != ty {
                    return Err(Error::Redefined(named.name().to_string()));
                }
                buf.push(TV_NAME_REF);
                put_name(buf, named.name());
            } else {
                defs.insert(named.name().to_string(), ty.clone());
                buf.push(TV_NAME_DEF);
                put_name(buf, named.name());
                append(buf, named.inner(), defs)?;
            }
        }
        Type::Record(fields) => {
            buf.push(TV_RECORD);
            put_uvarint(buf, fields.len() as u64);
            for f in fields.iter() {
                put_name(buf, &f.name);
                append(buf, &f.ty, defs)?;
            }
        }
        Type::Array(t) => {
            buf.push(TV_ARRAY);
            append(buf, t, defs)?;
        }
        Type::Set(t) => {
            buf.push(TV_SET);
            append(buf, t, defs)?;
        }
        Type::Map(kv) => {
            buf.push(TV_MAP);
            append(buf, &kv.0, defs)?;
            append(buf, &kv.1, defs)?;
        }
        Type::Union(types) => {
            buf.push(TV_UNION);
            put_uvarint(buf, types.len() as u64);
            for t in types.iter() {
                append(buf, t, defs)?;
            }
        }
        Type::Enum(symbols) => {
            buf.push(TV_ENUM);
            put_uvarint(buf, symbols.len() as u64);
            for s in symbols.iter() {
                put_name(buf, s);
            }
        }
        Type::Error(t) => {
            buf.push(TV_ERROR);
            append(buf, t, defs)?;
        }
        Type::Fusion(t) => {
            buf.push(TV_FUSION);
            append(buf, t, defs)?;
        }
        Type::Option(t) => {
            buf.push(TV_OPTION);
            append(buf, t, defs)?;
        }
        Type::Decimal(p, s) => {
            buf.push(TV_DECIMAL);
            put_uvarint(buf, u64::from(*p));
            put_uvarint(buf, u64::from(*s));
        }
    }
    Ok(())
}

/// Decodes the type value in `buf`, which must hold exactly one type value.
pub fn decode_type_value(buf: &[u8]) -> Result<Type> {
    let mut c = Cursor::new(buf);
    let t = decode(&mut c, &mut HashMap::new(), 0)?;
    if !c.is_empty() {
        return Err(Error::malformed("extra bytes after type value"));
    }
    Ok(t)
}

fn decode(c: &mut Cursor, defs: &mut HashMap<String, Type>, depth: usize) -> Result<Type> {
    if depth > MAX_TYPE_DEPTH {
        return Err(Error::malformed("type value nested too deeply"));
    }
    let depth = depth + 1;
    let code = c.byte()?;
    Ok(match code {
        TV_NAME_DEF => {
            let name = c.name()?;
            check_type_name(&name).map_err(|e| Error::malformed(e.to_string()))?;
            let named = NamedType::declare(name.clone());
            let t = Type::Named(named.clone());
            // The binding is local to the type value and may be referred to
            // by the underlying type.
            defs.insert(name, t.clone());
            let inner = decode(c, defs, depth)?;
            if matches!(inner, Type::Named(ref n) if !n.is_bound()) {
                return Err(Error::malformed(
                    "named type refers to itself through named types only",
                ));
            }
            named.bind(inner);
            t
        }
        TV_NAME_REF => {
            let name = c.name()?;
            defs.get(&name).cloned().ok_or_else(|| {
                Error::malformed(format!("reference to undefined type name {name:?}"))
            })?
        }
        TV_RECORD => {
            let n = c.len()?;
            let mut fields = Vec::with_capacity(n);
            for _ in 0..n {
                let name = c.name()?;
                fields.push(Field::new(name, decode(c, defs, depth)?));
            }
            Type::record(fields).map_err(|e| Error::malformed(e.to_string()))?
        }
        TV_ARRAY => Type::array(decode(c, defs, depth)?),
        TV_SET => Type::set(decode(c, defs, depth)?),
        TV_MAP => {
            let k = decode(c, defs, depth)?;
            Type::map(k, decode(c, defs, depth)?)
        }
        TV_UNION => {
            let n = c.len()?;
            let mut types = Vec::with_capacity(n);
            for _ in 0..n {
                let t = decode(c, defs, depth)?;
                if matches!(t, Type::Union(_)) {
                    return Err(Error::malformed("union member is a union"));
                }
                types.push(t);
            }
            if types.is_empty() {
                return Err(Error::malformed("union of no types"));
            }
            Type::Union(types.into())
        }
        TV_ENUM => {
            let n = c.len()?;
            let mut symbols = Vec::with_capacity(n);
            for _ in 0..n {
                symbols.push(c.name()?);
            }
            Type::enumeration(symbols)
        }
        TV_ERROR => Type::error(decode(c, defs, depth)?),
        TV_FUSION => Type::fusion(decode(c, defs, depth)?),
        TV_OPTION => {
            Type::option(decode(c, defs, depth)?).map_err(|e| Error::malformed(e.to_string()))?
        }
        TV_DECIMAL => {
            let p = c.uvarint_u32()?;
            Type::Decimal(p, c.uvarint_u32()?)
        }
        id => Type::Primitive(
            Primitive::from_id(u32::from(id))
                .ok_or_else(|| Error::malformed(format!("bad type value code {id}")))?,
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let point = Type::named(
            "Point",
            Type::record(vec![Field::new("x", Type::FLOAT64)]).unwrap(),
        )
        .unwrap();
        let t = Type::record(vec![
            Field::new("a", point.clone()),
            Field::new("b", Type::array(point)),
            Field::new("c", Type::union([Type::INT64, Type::NULL]).unwrap()),
            Field::new(
                "d",
                Type::map(Type::STRING, Type::option(Type::BYTES).unwrap()),
            ),
            Field::new("e", Type::fusion(Type::ALL)),
        ])
        .unwrap();
        let tv = encode_type_value(&t).unwrap();
        // The second occurrence of Point is a reference.
        assert_eq!(tv.iter().filter(|&&b| b == TV_NAME_REF).count(), 1);
        assert_eq!(decode_type_value(&tv).unwrap(), t);
        assert_eq!(
            decode_type_value(&[Primitive::String.id() as u8]).unwrap(),
            Type::STRING
        );
        assert!(decode_type_value(&[TV_NAME_REF, 1, b'x']).is_err());
    }
}
