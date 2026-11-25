//! Conversion of a value whose type was inferred from the value (see
//! [`crate::ser`]) to a value of a static type (see [`crate::schema`]).

use crate::error::{Error, Result};
use crate::types::{Primitive, Type};
use crate::value::Value;

/// Returns true if `t` is the named type of a variant of the enum `name`.
fn variant_of(t: &Type, name: &str) -> bool {
    matches!(t, Type::Named(n) if n.name().strip_prefix(name).is_some_and(|s| s.starts_with('_')))
}

fn mismatch(v: &Value, t: &Type) -> Error {
    Error::msg(format!("can't convert value {v} to type {t:?}"))
}

/// Returns the value of a field of type `t` that was skipped when its
/// record was serialized (e.g., with `skip_serializing_if`): none for an
/// option and null for a type that includes null.
fn absent(t: &Type) -> Option<Value> {
    match t.under() {
        Type::Option(_) => Some(Value::None),
        Type::Primitive(Primitive::Null) => Some(Value::Null),
        Type::Union(types) if types.contains(&Type::NULL) => Some(Value::Null),
        Type::Fusion(inner) if **inner == Type::ALL => Some(Value::Null),
        _ => None,
    }
}

/// Returns the value inside a union value (as produced for the elements of
/// a container of values of different types).
fn strip_union(v: Value) -> Value {
    match v {
        Value::Union(_, inner) => *inner,
        v => v,
    }
}

/// Converts `v` to a value of type `t`.
pub(crate) fn coerce(v: Value, t: &Type) -> Result<Value> {
    coerce_in(v, t, None)
}

/// Returns the tag of a variant in serde's externally tagged representation
/// of enums: a unit variant is its name and any other variant is a record
/// with a single field named for it.
fn tag(v: &Value) -> Option<&str> {
    match v {
        Value::String(s) => Some(s),
        Value::Record(fields) if fields.len() == 1 => Some(&fields[0].0),
        _ => None,
    }
}

/// Converts `v` to a value of type `t`, where `enum_name` is the name of the
/// enum whose variants `t` may be (whose named types are "Enum_Variant").
fn coerce_in(v: Value, t: &Type, enum_name: Option<&str>) -> Result<Value> {
    let v = match (strip_union(v), t) {
        // A present option where the static type isn't an option (e.g.,
        // inside an `Option<Option<T>>`, which is `option(T)`).
        (
            Value::Option(_, Some(inner)),
            Type::Primitive(_)
            | Type::Record(_)
            | Type::Array(_)
            | Type::Set(_)
            | Type::Map(_)
            | Type::Enum(_)
            | Type::Error(_),
        ) => *inner,
        (v, _) => v,
    };
    match t {
        Type::Primitive(_) => {
            if v.typ() == *t {
                Ok(v)
            } else {
                Err(mismatch(&v, t))
            }
        }
        Type::Fusion(inner) if **inner == Type::ALL => {
            let sub = v.typ();
            Ok(Value::Fusion(Type::ALL, Box::new(v), sub))
        }
        Type::Named(n) => {
            if matches!(&v, Value::Named(vn, _) if vn == n) {
                return Ok(v);
            }
            let inner = n.inner();
            // The payload of an enum variant.
            let variant = enum_name
                .and_then(|e| n.name().strip_prefix(e))
                .and_then(|s| s.strip_prefix('_'));
            if let Some(variant) = variant {
                if tag(&v) == Some(variant) {
                    let payload = match v {
                        Value::Record(mut fields) => fields.pop().unwrap().1,
                        _ => Value::Null,
                    };
                    return Ok(Value::Named(n.clone(), Box::new(coerce(payload, inner)?)));
                }
                return Err(mismatch(&v, t));
            }
            // The underlying type of an enum is a union of its variants
            // (or its only variant).
            let is_variants = |t: &Type| match t {
                Type::Union(members) => members.iter().all(|m| variant_of(m, n.name())),
                t => variant_of(t, n.name()),
            };
            let ctx = if is_variants(inner) {
                Some(n.name())
            } else {
                None
            };
            Ok(Value::Named(n.clone(), Box::new(coerce_in(v, inner, ctx)?)))
        }
        Type::Union(members) => {
            let vt = v.typ();
            if members.contains(&vt) {
                return Ok(Value::Union(t.clone(), Box::new(v)));
            }
            for m in members.iter() {
                if let Ok(c) = coerce_in(v.clone(), m, enum_name) {
                    return Ok(Value::Union(t.clone(), Box::new(c)));
                }
            }
            Err(mismatch(&v, t))
        }
        Type::Record(fields) => {
            let Value::Record(mut vals) = v else {
                return Err(mismatch(&v, t));
            };
            let mut out = Vec::with_capacity(fields.len());
            for f in fields.iter() {
                let val = match vals.iter().position(|(n, _)| *n == f.name) {
                    Some(k) => vals.remove(k).1,
                    None => absent(&f.ty).ok_or_else(|| {
                        Error::msg(format!("missing field {:?} of type {:?}", f.name, t))
                    })?,
                };
                out.push((f.name.clone(), coerce(val, &f.ty)?));
            }
            if let Some((name, _)) = vals.first() {
                return Err(Error::msg(format!("field {name:?} isn't in type {t:?}")));
            }
            Ok(Value::Record(out))
        }
        Type::Array(elem) => match v {
            Value::Array(_, vals) => Ok(Value::Array(
                (**elem).clone(),
                vals.into_iter()
                    .map(|v| coerce(v, elem))
                    .collect::<Result<_>>()?,
            )),
            v => Err(mismatch(&v, t)),
        },
        Type::Set(elem) => match v {
            Value::Array(_, vals) | Value::Set(_, vals) => Ok(Value::Set(
                (**elem).clone(),
                vals.into_iter()
                    .map(|v| coerce(v, elem))
                    .collect::<Result<_>>()?,
            )),
            v => Err(mismatch(&v, t)),
        },
        Type::Map(kv) => match v {
            Value::Map(_, _, entries) => Ok(Value::Map(
                kv.0.clone(),
                kv.1.clone(),
                entries
                    .into_iter()
                    .map(|(k, v)| Ok((coerce(k, &kv.0)?, coerce(v, &kv.1)?)))
                    .collect::<Result<_>>()?,
            )),
            v => Err(mismatch(&v, t)),
        },
        Type::Enum(symbols) => match &v {
            Value::String(s) => match symbols.iter().position(|x| x == s) {
                Some(k) => Ok(Value::Enum(symbols.clone(), k as u32)),
                None => Err(mismatch(&v, t)),
            },
            _ => Err(mismatch(&v, t)),
        },
        Type::Option(inner) => {
            let some = |v: Value| Value::Option((**inner).clone(), Some(Box::new(v)));
            let none = || Value::Option((**inner).clone(), None);
            match v {
                Value::None | Value::Option(_, None) => Ok(none()),
                Value::Option(_, Some(v)) => Ok(some(coerce(*v, inner)?)),
                // A null where the option's type has no null is an absent
                // value from a Serialize impl that writes a unit for None.
                Value::Null => Ok(coerce(Value::Null, inner).map_or_else(|_| none(), some)),
                v => Ok(some(coerce(v, inner)?)),
            }
        }
        Type::Error(inner) => match v {
            Value::Error(x) => Ok(Value::Error(Box::new(coerce(*x, inner)?))),
            v => Err(mismatch(&v, t)),
        },
        _ => Err(mismatch(&v, t)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Field;

    #[test]
    fn options_and_variants() {
        let opt = Type::option(Type::INT64).unwrap();
        let rec = Type::record(vec![
            Field::new("a", opt.clone()),
            Field::new("b", Type::STRING),
        ])
        .unwrap();
        let v = coerce(Value::record([("b", Value::String("x".into()))]), &rec).unwrap();
        assert_eq!(v.to_string(), r#"{a?:none::int64,b:"x"}"#);
        assert_eq!(v.typ(), rec);
        let circle = Type::named("Shape_Circle", Type::FLOAT64).unwrap();
        let empty = Type::named("Shape_Empty", Type::NULL).unwrap();
        let shape = Type::named("Shape", Type::union([circle, empty]).unwrap()).unwrap();
        let v = coerce(Value::record([("Circle", Value::Float64(1.5))]), &shape).unwrap();
        assert_eq!(v.typ(), shape);
        let v = coerce(Value::String("Empty".into()), &shape).unwrap();
        assert_eq!(v.typ(), shape);
        assert!(coerce(Value::String("Square".into()), &shape).is_err());
    }
}
