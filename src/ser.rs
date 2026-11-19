//! Serializing Rust values to super-structured values with serde.
//!
//! The [`Serializer`] here infers the type of each value from the value
//! itself (see [`crate::to_value_untyped`]): a struct is an anonymous
//! record, `None` is null, a sequence whose elements have different types
//! is an array of a union type, and an enum variant is represented as serde
//! represents it in self-describing formats like JSON (a unit variant is its
//! name and any other variant is a record with a single field named for the
//! variant).  Static types, with named types for structs and enums, are
//! applied afterward (see [`crate::to_value`] and [`crate::schema`]).

use std::fmt::Display;

use serde::ser::{self, Serialize};

use crate::error::Error;
use crate::types::{flatten, Type};
use crate::value::Value;

impl ser::Error for Error {
    fn custom<T: Display>(msg: T) -> Self {
        Error::Message(msg.to_string())
    }
}

/// The name of the newtype struct that [`Timestamp`] serializes as.
pub(crate) const TIMESTAMP_NAME: &str = "$superdb::Timestamp";
/// The name of the newtype struct that [`Duration`] serializes as.
pub(crate) const DURATION_NAME: &str = "$superdb::Duration";

/// A timestamp in nanoseconds since the Unix epoch, which serializes as a
/// value of type `timestamp` (and as an `i64` in other serde formats).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Timestamp(pub i64);

/// A duration in nanoseconds, which serializes as a value of type
/// `duration` (and as an `i64` in other serde formats).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Duration(pub i64);

impl Serialize for Timestamp {
    fn serialize<S: ser::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_newtype_struct(TIMESTAMP_NAME, &self.0)
    }
}

impl Serialize for Duration {
    fn serialize<S: ser::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_newtype_struct(DURATION_NAME, &self.0)
    }
}

macro_rules! newtype_deserialize {
    ($t:ident, $name:expr, $what:expr) => {
        impl<'de> serde::Deserialize<'de> for $t {
            fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                struct V;
                impl<'de> serde::de::Visitor<'de> for V {
                    type Value = $t;
                    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                        f.write_str($what)
                    }
                    fn visit_newtype_struct<D: serde::Deserializer<'de>>(
                        self,
                        d: D,
                    ) -> Result<$t, D::Error> {
                        <i64 as serde::Deserialize>::deserialize(d).map($t)
                    }
                    fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<$t, E> {
                        Ok($t(v))
                    }
                    fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<$t, E> {
                        i64::try_from(v)
                            .map($t)
                            .map_err(|_| E::custom(concat!($what, " out of range")))
                    }
                }
                d.deserialize_newtype_struct($name, V)
            }
        }
    };
}

newtype_deserialize!(Timestamp, TIMESTAMP_NAME, "a timestamp");
newtype_deserialize!(Duration, DURATION_NAME, "a duration");

/// A serde serializer whose output is a [`Value`].
pub struct Serializer;

/// Returns the union type of the elements of a container and the elements,
/// wrapped as union values if their types differ.  The element type of an
/// empty container is `none`, as is the element type of an empty container
/// among non-empty ones, which absorbs it.
pub(crate) fn unify(vals: Vec<Value>) -> (Type, Vec<Value>) {
    if vals.is_empty() {
        return (Type::NONE, vals);
    }
    let types: Vec<Type> = vals.iter().map(Value::typ).collect();
    let mut uniq = flatten(types.iter().cloned());
    if uniq.len() == 1 {
        return (uniq.pop().unwrap(), vals);
    }
    // An empty container's type yields to the other container types of its
    // kind (e.g., [none] to [int64]) when there's exactly one.
    let absorbed: Vec<Type> = uniq
        .iter()
        .filter(|t| {
            !(is_empty_container(t) && uniq.iter().filter(|u| same_container(u, t)).count() == 2)
        })
        .cloned()
        .collect();
    let vals: Vec<Value> = vals
        .into_iter()
        .map(|v| {
            let t = v.typ();
            if absorbed.contains(&t) {
                v
            } else {
                retype_empty(v, absorbed.iter().find(|u| same_container(u, &t)).unwrap())
            }
        })
        .collect();
    if absorbed.len() == 1 {
        return (absorbed[0].clone(), vals);
    }
    let union = Type::Union(absorbed.into());
    let vals = vals
        .into_iter()
        .map(|v| Value::Union(union.clone(), Box::new(v)))
        .collect();
    (union, vals)
}

fn is_empty_container(t: &Type) -> bool {
    match t {
        Type::Array(e) | Type::Set(e) => **e == Type::NONE,
        Type::Map(kv) => kv.0 == Type::NONE && kv.1 == Type::NONE,
        _ => false,
    }
}

fn same_container(a: &Type, b: &Type) -> bool {
    matches!(
        (a, b),
        (Type::Array(_), Type::Array(_))
            | (Type::Set(_), Type::Set(_))
            | (Type::Map(_), Type::Map(_))
    )
}

fn retype_empty(v: Value, t: &Type) -> Value {
    match (v, t) {
        (Value::Array(_, vals), Type::Array(e)) => Value::Array((**e).clone(), vals),
        (Value::Set(_, vals), Type::Set(e)) => Value::Set((**e).clone(), vals),
        (Value::Map(_, _, vals), Type::Map(kv)) => Value::Map(kv.0.clone(), kv.1.clone(), vals),
        (v, _) => v,
    }
}

fn tuple_record(vals: Vec<Value>) -> Value {
    Value::Record(
        vals.into_iter()
            .enumerate()
            .map(|(i, v)| (format!("column{}", i + 1), v))
            .collect(),
    )
}

fn ip_from_octets(v: &Value) -> Option<std::net::IpAddr> {
    let Value::Record(fields) = v else {
        return None;
    };
    let octets: Vec<u8> = fields
        .iter()
        .map(|(_, v)| match v {
            Value::Uint8(b) => Some(*b),
            _ => None,
        })
        .collect::<Option<_>>()?;
    match octets.len() {
        4 => Some(std::net::IpAddr::from(<[u8; 4]>::try_from(octets).ok()?)),
        16 => Some(std::net::IpAddr::from(<[u8; 16]>::try_from(octets).ok()?)),
        _ => None,
    }
}

impl ser::Serializer for Serializer {
    type Ok = Value;
    type Error = Error;
    type SerializeSeq = SeqSerializer;
    type SerializeTuple = TupleSerializer;
    type SerializeTupleStruct = TupleSerializer;
    type SerializeTupleVariant = TupleSerializer;
    type SerializeMap = MapSerializer;
    type SerializeStruct = StructSerializer;
    type SerializeStructVariant = StructSerializer;

    fn serialize_bool(self, v: bool) -> Result<Value, Error> {
        Ok(Value::Bool(v))
    }
    fn serialize_i8(self, v: i8) -> Result<Value, Error> {
        Ok(Value::Int8(v))
    }
    fn serialize_i16(self, v: i16) -> Result<Value, Error> {
        Ok(Value::Int16(v))
    }
    fn serialize_i32(self, v: i32) -> Result<Value, Error> {
        Ok(Value::Int32(v))
    }
    fn serialize_i64(self, v: i64) -> Result<Value, Error> {
        Ok(Value::Int64(v))
    }
    fn serialize_i128(self, v: i128) -> Result<Value, Error> {
        i64::try_from(v)
            .map(Value::Int64)
            .map_err(|_| Error::msg("i128 value out of the range of int64"))
    }
    fn serialize_u8(self, v: u8) -> Result<Value, Error> {
        Ok(Value::Uint8(v))
    }
    fn serialize_u16(self, v: u16) -> Result<Value, Error> {
        Ok(Value::Uint16(v))
    }
    fn serialize_u32(self, v: u32) -> Result<Value, Error> {
        Ok(Value::Uint32(v))
    }
    fn serialize_u64(self, v: u64) -> Result<Value, Error> {
        Ok(Value::Uint64(v))
    }
    fn serialize_u128(self, v: u128) -> Result<Value, Error> {
        u64::try_from(v)
            .map(Value::Uint64)
            .map_err(|_| Error::msg("u128 value out of the range of uint64"))
    }
    fn serialize_f32(self, v: f32) -> Result<Value, Error> {
        Ok(Value::Float32(v))
    }
    fn serialize_f64(self, v: f64) -> Result<Value, Error> {
        Ok(Value::Float64(v))
    }
    fn serialize_char(self, v: char) -> Result<Value, Error> {
        Ok(Value::String(v.to_string()))
    }
    fn serialize_str(self, v: &str) -> Result<Value, Error> {
        Ok(Value::String(v.to_string()))
    }
    fn serialize_bytes(self, v: &[u8]) -> Result<Value, Error> {
        Ok(Value::Bytes(v.to_vec()))
    }
    fn serialize_none(self) -> Result<Value, Error> {
        Ok(Value::Null)
    }
    fn serialize_some<T: ?Sized + Serialize>(self, value: &T) -> Result<Value, Error> {
        value.serialize(self)
    }
    fn serialize_unit(self) -> Result<Value, Error> {
        Ok(Value::Null)
    }
    fn serialize_unit_struct(self, _name: &'static str) -> Result<Value, Error> {
        Ok(Value::Null)
    }
    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
    ) -> Result<Value, Error> {
        Ok(Value::String(variant.to_string()))
    }
    fn serialize_newtype_struct<T: ?Sized + Serialize>(
        self,
        name: &'static str,
        value: &T,
    ) -> Result<Value, Error> {
        let v = value.serialize(Serializer)?;
        match (name, v) {
            (TIMESTAMP_NAME, Value::Int64(n)) => Ok(Value::Timestamp(n)),
            (DURATION_NAME, Value::Int64(n)) => Ok(Value::Duration(n)),
            (_, v) => Ok(v),
        }
    }
    fn serialize_newtype_variant<T: ?Sized + Serialize>(
        self,
        name: &'static str,
        _index: u32,
        variant: &'static str,
        value: &T,
    ) -> Result<Value, Error> {
        let v = value.serialize(Serializer)?;
        // std::net::IpAddr serializes as an enum of its octets in formats
        // that aren't human readable.
        if name == "IpAddr" && (variant == "V4" || variant == "V6") {
            if let Some(ip) = ip_from_octets(&v) {
                return Ok(Value::Ip(ip));
            }
        }
        Ok(Value::record([(variant, v)]))
    }
    fn serialize_seq(self, len: Option<usize>) -> Result<SeqSerializer, Error> {
        Ok(SeqSerializer {
            vals: Vec::with_capacity(len.unwrap_or(0)),
        })
    }
    fn serialize_tuple(self, len: usize) -> Result<TupleSerializer, Error> {
        Ok(TupleSerializer {
            vals: Vec::with_capacity(len),
            variant: None,
        })
    }
    fn serialize_tuple_struct(
        self,
        _name: &'static str,
        len: usize,
    ) -> Result<TupleSerializer, Error> {
        self.serialize_tuple(len)
    }
    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<TupleSerializer, Error> {
        Ok(TupleSerializer {
            vals: Vec::with_capacity(len),
            variant: Some(variant),
        })
    }
    fn serialize_map(self, len: Option<usize>) -> Result<MapSerializer, Error> {
        Ok(MapSerializer {
            entries: Vec::with_capacity(len.unwrap_or(0)),
            key: None,
        })
    }
    fn serialize_struct(self, _name: &'static str, len: usize) -> Result<StructSerializer, Error> {
        Ok(StructSerializer {
            fields: Vec::with_capacity(len),
            variant: None,
        })
    }
    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<StructSerializer, Error> {
        Ok(StructSerializer {
            fields: Vec::with_capacity(len),
            variant: Some(variant),
        })
    }
    fn is_human_readable(&self) -> bool {
        false
    }
}

#[doc(hidden)]
pub struct SeqSerializer {
    vals: Vec<Value>,
}

impl ser::SerializeSeq for SeqSerializer {
    type Ok = Value;
    type Error = Error;
    fn serialize_element<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), Error> {
        self.vals.push(value.serialize(Serializer)?);
        Ok(())
    }
    fn end(self) -> Result<Value, Error> {
        let (elem, vals) = unify(self.vals);
        Ok(Value::Array(elem, vals))
    }
}

#[doc(hidden)]
pub struct TupleSerializer {
    vals: Vec<Value>,
    variant: Option<&'static str>,
}

impl TupleSerializer {
    fn push<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), Error> {
        self.vals.push(value.serialize(Serializer)?);
        Ok(())
    }

    fn finish(self) -> Value {
        let rec = tuple_record(self.vals);
        match self.variant {
            Some(v) => Value::record([(v, rec)]),
            None => rec,
        }
    }
}

impl ser::SerializeTuple for TupleSerializer {
    type Ok = Value;
    type Error = Error;
    fn serialize_element<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), Error> {
        self.push(value)
    }
    fn end(self) -> Result<Value, Error> {
        Ok(self.finish())
    }
}

impl ser::SerializeTupleStruct for TupleSerializer {
    type Ok = Value;
    type Error = Error;
    fn serialize_field<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), Error> {
        self.push(value)
    }
    fn end(self) -> Result<Value, Error> {
        Ok(self.finish())
    }
}

impl ser::SerializeTupleVariant for TupleSerializer {
    type Ok = Value;
    type Error = Error;
    fn serialize_field<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), Error> {
        self.push(value)
    }
    fn end(self) -> Result<Value, Error> {
        Ok(self.finish())
    }
}

#[doc(hidden)]
pub struct MapSerializer {
    entries: Vec<(Value, Value)>,
    key: Option<Value>,
}

impl ser::SerializeMap for MapSerializer {
    type Ok = Value;
    type Error = Error;
    fn serialize_key<T: ?Sized + Serialize>(&mut self, key: &T) -> Result<(), Error> {
        self.key = Some(key.serialize(Serializer)?);
        Ok(())
    }
    fn serialize_value<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), Error> {
        let key = self
            .key
            .take()
            .ok_or_else(|| Error::msg("map value without a key"))?;
        self.entries.push((key, value.serialize(Serializer)?));
        Ok(())
    }
    fn end(self) -> Result<Value, Error> {
        let (keys, vals): (Vec<Value>, Vec<Value>) = self.entries.into_iter().unzip();
        let (kt, keys) = unify(keys);
        let (vt, vals) = unify(vals);
        Ok(Value::Map(kt, vt, keys.into_iter().zip(vals).collect()))
    }
}

#[doc(hidden)]
pub struct StructSerializer {
    fields: Vec<(String, Value)>,
    variant: Option<&'static str>,
}

impl StructSerializer {
    fn push<T: ?Sized + Serialize>(&mut self, key: &'static str, value: &T) -> Result<(), Error> {
        self.fields
            .push((key.to_string(), value.serialize(Serializer)?));
        Ok(())
    }

    fn finish(self) -> Value {
        let rec = Value::Record(self.fields);
        match self.variant {
            Some(v) => Value::record([(v, rec)]),
            None => rec,
        }
    }
}

impl ser::SerializeStruct for StructSerializer {
    type Ok = Value;
    type Error = Error;
    fn serialize_field<T: ?Sized + Serialize>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), Error> {
        self.push(key, value)
    }
    fn end(self) -> Result<Value, Error> {
        Ok(self.finish())
    }
}

impl ser::SerializeStructVariant for StructSerializer {
    type Ok = Value;
    type Error = Error;
    fn serialize_field<T: ?Sized + Serialize>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), Error> {
        self.push(key, value)
    }
    fn end(self) -> Result<Value, Error> {
        Ok(self.finish())
    }
}
