//! Deserializing Rust values from super-structured values with serde.
//!
//! The deserializer accepts the values written by this crate's serializer
//! (with static types or inferred types) and, more generally, any value of a
//! compatible shape: a struct from a record (whose absent and missing fields
//! are left to serde's defaults), an `Option` from a nullable value, an enum
//! from a value of a named type `Enum_Variant`, an enum value, a string
//! naming a unit variant, or a record with a single field naming the
//! variant, and so on.  Values of union, named, option, and fusion types are
//! deserialized from the values inside them.

use serde::de::{self, value::BorrowedStrDeserializer, DeserializeSeed, Visitor};

use crate::error::{Error, Result};
use crate::value::Value;

macro_rules! forward_to_any {
    ($($method:ident)*) => {$(
        fn $method<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
            self.deserialize_any(visitor)
        }
    )*};
}

/// A serde deserializer of a [`Value`].
pub struct Deserializer<'de> {
    v: &'de Value,
}

impl<'de> Deserializer<'de> {
    /// Returns a deserializer of `v`.
    pub fn new(v: &'de Value) -> Self {
        Deserializer { v }
    }
}

/// Returns the value inside union, option, and fusion values, and also
/// inside named values if `named` is true.
fn peel(mut v: &Value, named: bool) -> &Value {
    loop {
        v = match v {
            Value::Union(_, inner) | Value::Option(_, Some(inner)) | Value::Fusion(_, inner, _) => {
                inner
            }
            Value::Named(_, inner) if named => inner,
            _ => return v,
        };
    }
}

fn is_absent(v: &Value) -> bool {
    matches!(
        peel(v, true),
        Value::Null | Value::Missing | Value::None | Value::Option(_, None)
    )
}

fn unexpected(v: &Value) -> de::Unexpected<'_> {
    match v {
        Value::Bool(b) => de::Unexpected::Bool(*b),
        Value::Uint8(_) | Value::Uint16(_) | Value::Uint32(_) | Value::Uint64(_) => {
            de::Unexpected::Other("unsigned integer")
        }
        Value::Int8(_) | Value::Int16(_) | Value::Int32(_) | Value::Int64(_) => {
            de::Unexpected::Other("integer")
        }
        Value::Float16(_) | Value::Float32(_) | Value::Float64(_) => de::Unexpected::Other("float"),
        Value::String(s) => de::Unexpected::Str(s),
        Value::Bytes(b) => de::Unexpected::Bytes(b),
        Value::Null | Value::Missing | Value::None | Value::Option(_, None) => de::Unexpected::Unit,
        Value::Record(_) | Value::Map(..) => de::Unexpected::Map,
        Value::Array(..) | Value::Set(..) => de::Unexpected::Seq,
        _ => de::Unexpected::Other("value"),
    }
}

impl<'de> de::Deserializer<'de> for Deserializer<'de> {
    type Error = Error;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        let v = peel(self.v, true);
        match v {
            Value::Uint8(x) => visitor.visit_u8(*x),
            Value::Uint16(x) => visitor.visit_u16(*x),
            Value::Uint32(x) => visitor.visit_u32(*x),
            Value::Uint64(x) => visitor.visit_u64(*x),
            Value::Int8(x) => visitor.visit_i8(*x),
            Value::Int16(x) => visitor.visit_i16(*x),
            Value::Int32(x) => visitor.visit_i32(*x),
            Value::Int64(x) | Value::Duration(x) | Value::Timestamp(x) => visitor.visit_i64(*x),
            Value::Float16(x) | Value::Float32(x) => visitor.visit_f32(*x),
            Value::Float64(x) => visitor.visit_f64(*x),
            Value::Bool(x) => visitor.visit_bool(*x),
            Value::Bytes(b) | Value::Reserved(_, b) => visitor.visit_borrowed_bytes(b),
            Value::String(s) => visitor.visit_borrowed_str(s),
            Value::Ip(_) | Value::Net(..) | Value::Uuid(_) | Value::Type(_) => {
                visitor.visit_string(v.to_string())
            }
            Value::Null | Value::Missing | Value::None | Value::Option(_, None) => {
                visitor.visit_unit()
            }
            Value::Record(fields) => visitor.visit_map(RecordAccess::new(fields)),
            Value::Array(_, vals) | Value::Set(_, vals) => {
                visitor.visit_seq(SeqAccess { it: vals.iter() })
            }
            Value::Map(_, _, entries) => visitor.visit_map(MapAccess {
                it: entries.iter(),
                val: None,
            }),
            Value::Enum(symbols, k) => visitor.visit_borrowed_str(&symbols[*k as usize]),
            Value::Error(inner) => Err(Error::msg(format!("error value: {inner}"))),
            Value::Union(..) | Value::Named(..) | Value::Option(..) | Value::Fusion(..) => {
                unreachable!()
            }
        }
    }

    fn deserialize_bool<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.deserialize_any(visitor)
    }
    fn deserialize_i8<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.deserialize_any(visitor)
    }
    fn deserialize_i16<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.deserialize_any(visitor)
    }
    fn deserialize_i32<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.deserialize_any(visitor)
    }
    fn deserialize_i64<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.deserialize_any(visitor)
    }
    fn deserialize_u8<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.deserialize_any(visitor)
    }
    fn deserialize_u16<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.deserialize_any(visitor)
    }
    fn deserialize_u32<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.deserialize_any(visitor)
    }
    fn deserialize_u64<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.deserialize_any(visitor)
    }
    fn deserialize_f32<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.deserialize_any(visitor)
    }
    fn deserialize_f64<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.deserialize_any(visitor)
    }
    fn deserialize_char<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.deserialize_any(visitor)
    }
    fn deserialize_str<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.deserialize_any(visitor)
    }
    fn deserialize_string<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.deserialize_any(visitor)
    }
    fn deserialize_bytes<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        match peel(self.v, true) {
            Value::String(s) => visitor.visit_borrowed_bytes(s.as_bytes()),
            Value::Uuid(u) => visitor.visit_borrowed_bytes(u),
            _ => self.deserialize_any(visitor),
        }
    }
    fn deserialize_byte_buf<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.deserialize_bytes(visitor)
    }

    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        if is_absent(self.v) {
            visitor.visit_none()
        } else {
            visitor.visit_some(Deserializer {
                v: peel(self.v, false),
            })
        }
    }

    fn deserialize_unit<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        if is_absent(self.v) {
            visitor.visit_unit()
        } else {
            Err(de::Error::invalid_type(
                unexpected(peel(self.v, true)),
                &visitor,
            ))
        }
    }

    fn deserialize_unit_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value> {
        self.deserialize_unit(visitor)
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value> {
        visitor.visit_newtype_struct(Deserializer {
            v: peel(self.v, true),
        })
    }

    fn deserialize_seq<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        match peel(self.v, true) {
            Value::Array(_, vals) | Value::Set(_, vals) => {
                visitor.visit_seq(SeqAccess { it: vals.iter() })
            }
            Value::Record(fields) => visitor.visit_seq(FieldValues { it: fields.iter() }),
            _ => self.deserialize_any(visitor),
        }
    }

    fn deserialize_tuple<V: Visitor<'de>>(self, _len: usize, visitor: V) -> Result<V::Value> {
        self.deserialize_seq(visitor)
    }

    fn deserialize_tuple_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _len: usize,
        visitor: V,
    ) -> Result<V::Value> {
        self.deserialize_seq(visitor)
    }

    fn deserialize_map<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.deserialize_any(visitor)
    }

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value> {
        self.deserialize_any(visitor)
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        name: &'static str,
        variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value> {
        let mut v = peel(self.v, false);
        loop {
            match v {
                Value::Named(n, inner) => {
                    // The value of a variant has the named type
                    // "Enum_Variant" and the value of the enum may have the
                    // named type "Enum".
                    let variant = n
                        .name()
                        .strip_prefix(name)
                        .and_then(|s| s.strip_prefix('_'))
                        .and_then(|s| variants.iter().find(|v| **v == s));
                    if let Some(variant) = variant {
                        return visitor.visit_enum(EnumAccess {
                            variant,
                            payload: Some(inner),
                        });
                    }
                    v = peel(inner, false);
                }
                Value::Enum(symbols, k) => {
                    return visitor.visit_enum(EnumAccess {
                        variant: &symbols[*k as usize],
                        payload: None,
                    });
                }
                Value::String(s) => {
                    return visitor.visit_enum(EnumAccess {
                        variant: s,
                        payload: None,
                    })
                }
                Value::Record(fields) if fields.len() == 1 => {
                    return visitor.visit_enum(EnumAccess {
                        variant: &fields[0].0,
                        payload: Some(&fields[0].1),
                    });
                }
                Value::Map(_, _, entries) if entries.len() == 1 => {
                    match peel(&entries[0].0, true) {
                        Value::String(s) => {
                            return visitor.visit_enum(EnumAccess {
                                variant: s,
                                payload: Some(&entries[0].1),
                            })
                        }
                        other => return Err(de::Error::invalid_type(unexpected(other), &visitor)),
                    }
                }
                Value::Ip(ip) if name == "IpAddr" => {
                    return visitor.visit_enum(IpAccess { ip: *ip });
                }
                other => return Err(de::Error::invalid_type(unexpected(other), &visitor)),
            }
        }
    }

    fn deserialize_identifier<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.deserialize_any(visitor)
    }

    fn deserialize_ignored_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        visitor.visit_unit()
    }

    fn is_human_readable(&self) -> bool {
        false
    }
}

struct SeqAccess<'de> {
    it: std::slice::Iter<'de, Value>,
}

impl<'de> de::SeqAccess<'de> for SeqAccess<'de> {
    type Error = Error;

    fn next_element_seed<T: DeserializeSeed<'de>>(&mut self, seed: T) -> Result<Option<T::Value>> {
        match self.it.next() {
            Some(v) => seed.deserialize(Deserializer { v }).map(Some),
            None => Ok(None),
        }
    }

    fn size_hint(&self) -> Option<usize> {
        Some(self.it.len())
    }
}

/// Accesses the field values of a record as a sequence (e.g., a tuple).
struct FieldValues<'de> {
    it: std::slice::Iter<'de, (String, Value)>,
}

impl<'de> de::SeqAccess<'de> for FieldValues<'de> {
    type Error = Error;

    fn next_element_seed<T: DeserializeSeed<'de>>(&mut self, seed: T) -> Result<Option<T::Value>> {
        match self.it.next() {
            Some((_, v)) => seed.deserialize(Deserializer { v }).map(Some),
            None => Ok(None),
        }
    }
}

/// Accesses the fields of a record as a map, skipping missing fields.
struct RecordAccess<'de> {
    it: std::slice::Iter<'de, (String, Value)>,
    val: Option<&'de Value>,
}

impl<'de> RecordAccess<'de> {
    fn new(fields: &'de [(String, Value)]) -> Self {
        RecordAccess {
            it: fields.iter(),
            val: None,
        }
    }
}

impl<'de> de::MapAccess<'de> for RecordAccess<'de> {
    type Error = Error;

    fn next_key_seed<K: DeserializeSeed<'de>>(&mut self, seed: K) -> Result<Option<K::Value>> {
        for (name, v) in self.it.by_ref() {
            if matches!(peel(v, true), Value::Missing) {
                continue;
            }
            self.val = Some(v);
            return seed
                .deserialize(BorrowedStrDeserializer::<Error>::new(name.as_str()))
                .map(Some);
        }
        Ok(None)
    }

    fn next_value_seed<V: DeserializeSeed<'de>>(&mut self, seed: V) -> Result<V::Value> {
        let v = self
            .val
            .take()
            .ok_or_else(|| Error::msg("record value without a field name"))?;
        seed.deserialize(Deserializer { v })
    }
}

struct MapAccess<'de> {
    it: std::slice::Iter<'de, (Value, Value)>,
    val: Option<&'de Value>,
}

impl<'de> de::MapAccess<'de> for MapAccess<'de> {
    type Error = Error;

    fn next_key_seed<K: DeserializeSeed<'de>>(&mut self, seed: K) -> Result<Option<K::Value>> {
        match self.it.next() {
            Some((k, v)) => {
                self.val = Some(v);
                seed.deserialize(Deserializer { v: k }).map(Some)
            }
            None => Ok(None),
        }
    }

    fn next_value_seed<V: DeserializeSeed<'de>>(&mut self, seed: V) -> Result<V::Value> {
        let v = self
            .val
            .take()
            .ok_or_else(|| Error::msg("map value without a key"))?;
        seed.deserialize(Deserializer { v })
    }
}

struct EnumAccess<'de> {
    variant: &'de str,
    payload: Option<&'de Value>,
}

impl<'de> de::EnumAccess<'de> for EnumAccess<'de> {
    type Error = Error;
    type Variant = VariantAccess<'de>;

    fn variant_seed<V: DeserializeSeed<'de>>(
        self,
        seed: V,
    ) -> Result<(V::Value, VariantAccess<'de>)> {
        let v = seed.deserialize(BorrowedStrDeserializer::<Error>::new(self.variant))?;
        Ok((
            v,
            VariantAccess {
                payload: self.payload,
            },
        ))
    }
}

struct VariantAccess<'de> {
    payload: Option<&'de Value>,
}

impl<'de> de::VariantAccess<'de> for VariantAccess<'de> {
    type Error = Error;

    fn unit_variant(self) -> Result<()> {
        match self.payload {
            None => Ok(()),
            Some(v) if is_absent(v) => Ok(()),
            Some(v) => Err(de::Error::invalid_type(
                unexpected(peel(v, true)),
                &"unit variant",
            )),
        }
    }

    fn newtype_variant_seed<T: DeserializeSeed<'de>>(self, seed: T) -> Result<T::Value> {
        match self.payload {
            Some(v) => seed.deserialize(Deserializer { v }),
            None => Err(de::Error::invalid_type(
                de::Unexpected::UnitVariant,
                &"newtype variant",
            )),
        }
    }

    fn tuple_variant<V: Visitor<'de>>(self, len: usize, visitor: V) -> Result<V::Value> {
        match self.payload {
            Some(v) => de::Deserializer::deserialize_tuple(Deserializer { v }, len, visitor),
            None => Err(de::Error::invalid_type(
                de::Unexpected::UnitVariant,
                &visitor,
            )),
        }
    }

    fn struct_variant<V: Visitor<'de>>(
        self,
        fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value> {
        match self.payload {
            Some(v) => {
                de::Deserializer::deserialize_struct(Deserializer { v }, "", fields, visitor)
            }
            None => Err(de::Error::invalid_type(
                de::Unexpected::UnitVariant,
                &visitor,
            )),
        }
    }
}

/// Accesses an IP address as the enum that `std::net::IpAddr` deserializes
/// from in formats that aren't human readable.
struct IpAccess {
    ip: std::net::IpAddr,
}

impl<'de> de::EnumAccess<'de> for IpAccess {
    type Error = Error;
    type Variant = Self;

    fn variant_seed<V: DeserializeSeed<'de>>(self, seed: V) -> Result<(V::Value, Self)> {
        let variant = if self.ip.is_ipv4() { "V4" } else { "V6" };
        Ok((
            seed.deserialize(BorrowedStrDeserializer::<Error>::new(variant))?,
            self,
        ))
    }
}

impl<'de> de::VariantAccess<'de> for IpAccess {
    type Error = Error;

    fn unit_variant(self) -> Result<()> {
        Err(Error::msg("expected an IP address"))
    }

    fn newtype_variant_seed<T: DeserializeSeed<'de>>(self, seed: T) -> Result<T::Value> {
        let octets = match self.ip {
            std::net::IpAddr::V4(a) => a.octets().to_vec(),
            std::net::IpAddr::V6(a) => a.octets().to_vec(),
        };
        seed.deserialize(Octets { octets })
    }

    fn tuple_variant<V: Visitor<'de>>(self, _len: usize, _visitor: V) -> Result<V::Value> {
        Err(Error::msg("expected an IP address"))
    }

    fn struct_variant<V: Visitor<'de>>(
        self,
        _fields: &'static [&'static str],
        _visitor: V,
    ) -> Result<V::Value> {
        Err(Error::msg("expected an IP address"))
    }
}

/// Deserializes the octets of an IP address as a tuple of `u8`.
struct Octets {
    octets: Vec<u8>,
}

impl<'de> de::Deserializer<'de> for Octets {
    type Error = Error;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        visitor.visit_seq(OctetSeq {
            it: self.octets.into_iter(),
        })
    }

    fn is_human_readable(&self) -> bool {
        false
    }

    forward_to_any! {
        deserialize_bool deserialize_i8 deserialize_i16 deserialize_i32 deserialize_i64 deserialize_u8
        deserialize_u16 deserialize_u32 deserialize_u64 deserialize_f32 deserialize_f64 deserialize_char
        deserialize_str deserialize_string deserialize_bytes deserialize_byte_buf deserialize_option
        deserialize_unit deserialize_seq deserialize_map deserialize_identifier deserialize_ignored_any
    }

    fn deserialize_unit_struct<V: Visitor<'de>>(self, _: &'static str, v: V) -> Result<V::Value> {
        self.deserialize_any(v)
    }
    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        _: &'static str,
        v: V,
    ) -> Result<V::Value> {
        self.deserialize_any(v)
    }
    fn deserialize_tuple<V: Visitor<'de>>(self, _: usize, v: V) -> Result<V::Value> {
        self.deserialize_any(v)
    }
    fn deserialize_tuple_struct<V: Visitor<'de>>(
        self,
        _: &'static str,
        _: usize,
        v: V,
    ) -> Result<V::Value> {
        self.deserialize_any(v)
    }
    fn deserialize_struct<V: Visitor<'de>>(
        self,
        _: &'static str,
        _: &'static [&'static str],
        v: V,
    ) -> Result<V::Value> {
        self.deserialize_any(v)
    }
    fn deserialize_enum<V: Visitor<'de>>(
        self,
        _: &'static str,
        _: &'static [&'static str],
        v: V,
    ) -> Result<V::Value> {
        self.deserialize_any(v)
    }
}

struct OctetSeq {
    it: std::vec::IntoIter<u8>,
}

impl<'de> de::SeqAccess<'de> for OctetSeq {
    type Error = Error;

    fn next_element_seed<T: DeserializeSeed<'de>>(&mut self, seed: T) -> Result<Option<T::Value>> {
        match self.it.next() {
            Some(b) => seed
                .deserialize(serde::de::value::U32Deserializer::<Error>::new(u32::from(
                    b,
                )))
                .map(Some),
            None => Ok(None),
        }
    }
}
