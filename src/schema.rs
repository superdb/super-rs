//! Static super-structured types of Rust types, discovered by tracing their
//! serde `Deserialize` implementations.
//!
//! Rust has no runtime reflection, but a type's `Deserialize` impl
//! describes the type's shape to the deserializer it's given: a struct's
//! name and field names, an enum's name and all of its variants, and the
//! element type of an `Option` or a `Vec`.  [`type_of`] runs a type's
//! `Deserialize` impl against a tracing deserializer that records that shape
//! (choosing a different enum variant on each pass until every variant has
//! been seen) and converts it to a [`Type`]:
//!
//! | Rust                         | super-structured type                     |
//! |------------------------------|-------------------------------------------|
//! | `bool`, `i8`..`i64`, `u8`..`u64`, `f32`, `f64` | `bool`, `int8`..`int64`, `uint8`..`uint64`, `float32`, `float64` |
//! | `char`, `String`, `&str`     | `string`                                  |
//! | bytes (e.g., `serde_bytes`)  | `bytes`                                   |
//! | `()`                         | `null`                                    |
//! | `Option<T>`                  | `T\|null`                                 |
//! | `Vec<T>` and other sequences | `[T]`                                     |
//! | maps                         | `map{K:V}`                                |
//! | tuples                       | `{column1:A,column2:B,...}`               |
//! | `struct S {..}`              | `S={...}`                                 |
//! | `struct S(A, B)`             | `S={column1:A,column2:B}`                 |
//! | `struct S(T)`                | `S=T`                                     |
//! | `struct S;`                  | `S=null`                                  |
//! | `enum E {A, B}` (unit variants only) | `E=enum(A,B)`                     |
//! | `enum E {A(T), B{x:X}, C}`   | `E=E_A\|E_B\|E_C` with `E_A=T`, `E_B={x:X}`, `E_C=null` |
//! | `std::net::IpAddr`           | `ip`                                      |
//! | [`Timestamp`], [`Duration`]  | `timestamp`, `duration`                   |
//! | anything traced by `deserialize_any` | `fusion(all)` (any type)          |
//!
//! [`Timestamp`]: crate::Timestamp
//! [`Duration`]: crate::Duration

use std::collections::{HashMap, HashSet};
use std::fmt::Display;

use serde::de::{
    self, value::BorrowedStrDeserializer, value::U32Deserializer, DeserializeSeed, Visitor,
};

use crate::error::{Error, Result};
use crate::ser::{DURATION_NAME, TIMESTAMP_NAME};
use crate::types::{Field, NamedType, Type};

#[derive(Clone, Debug, PartialEq)]
enum Format {
    Unknown,
    Any,
    Prim(Type),
    Option(Box<Format>),
    Seq(Box<Format>),
    Map(Box<Format>, Box<Format>),
    Tuple(Vec<Format>),
    Named(&'static str),
}

#[derive(Clone, Debug, PartialEq)]
enum Shape {
    Unit,
    Newtype(Format),
    Tuple(Vec<Format>),
    Struct(Vec<(&'static str, Format)>),
}

#[derive(Clone, Debug)]
enum Container {
    Shape(Shape),
    Enum(Vec<(&'static str, Option<Shape>)>),
}

/// Returns the error for a type name that refers to two different Rust
/// types (e.g., two instances of a generic struct or structs of the same
/// name in different modules), which can be renamed with
/// `#[serde(rename = "...")]`.
fn conflict(name: impl Display) -> Error {
    Error::Redefined(name.to_string())
}

fn merge(a: &mut Format, b: Format, name: &str) -> Result<()> {
    match (&mut *a, b) {
        (_, Format::Unknown) => {}
        (Format::Unknown, b) => *a = b,
        (Format::Any, _) => {}
        (_, Format::Any) => *a = Format::Any,
        (Format::Option(x), Format::Option(y)) | (Format::Seq(x), Format::Seq(y)) => {
            merge(x, *y, name)?
        }
        (Format::Map(k1, v1), Format::Map(k2, v2)) => {
            merge(k1, *k2, name)?;
            merge(v1, *v2, name)?;
        }
        (Format::Tuple(x), Format::Tuple(y)) if x.len() == y.len() => {
            for (x, y) in x.iter_mut().zip(y) {
                merge(x, y, name)?;
            }
        }
        (x, y) => {
            if *x != y {
                return Err(conflict(name));
            }
        }
    }
    Ok(())
}

fn merge_shape(a: &mut Shape, b: Shape, name: &str) -> Result<()> {
    match (a, b) {
        (Shape::Unit, Shape::Unit) => Ok(()),
        (Shape::Newtype(x), Shape::Newtype(y)) => merge(x, y, name),
        (Shape::Tuple(x), Shape::Tuple(y)) if x.len() == y.len() => {
            x.iter_mut().zip(y).try_for_each(|(x, y)| merge(x, y, name))
        }
        (Shape::Struct(x), Shape::Struct(y))
            if x.len() == y.len() && x.iter().zip(&y).all(|(f, g)| f.0 == g.0) =>
        {
            x.iter_mut()
                .zip(y)
                .try_for_each(|(x, y)| merge(&mut x.1, y.1, name))
        }
        _ => Err(conflict(name)),
    }
}

fn shape_known(s: &Shape) -> bool {
    fn known(f: &Format) -> bool {
        match f {
            Format::Unknown => false,
            Format::Option(x) | Format::Seq(x) => known(x),
            Format::Map(k, v) => known(k) && known(v),
            Format::Tuple(fs) => fs.iter().all(known),
            _ => true,
        }
    }
    match s {
        Shape::Unit => true,
        Shape::Newtype(f) => known(f),
        Shape::Tuple(fs) => fs.iter().all(known),
        Shape::Struct(fs) => fs.iter().all(|(_, f)| known(f)),
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Step {
    Slot(&'static str, u32, u32),
    Inner(u32),
}

#[derive(Default)]
struct State {
    registry: HashMap<&'static str, Container>,
    in_progress: Vec<&'static str>,
    pos: Vec<Step>,
    /// Positions of options, sequences, and maps whose values lead back to
    /// a container being traced, which are traced as having no value, with
    /// the format of their values.
    cut: HashMap<Vec<Step>, Format>,
    /// Set when a container is reached while it's being traced (i.e., the
    /// type is recursive) until the innermost option, sequence, or map
    /// leading to it is cut.
    recursion: bool,
    /// The enum variants chosen on the current path.
    choices: Vec<(&'static str, usize)>,
    /// Enum variants whose tracing led back to a container being traced, so
    /// other variants are traced first.
    deferred: HashSet<(&'static str, usize)>,
    progress: bool,
}

impl State {
    fn key(&self) -> Vec<Step> {
        let start = self
            .pos
            .iter()
            .rposition(|s| matches!(s, Step::Slot(..)))
            .unwrap_or(0);
        self.pos[start..].to_vec()
    }

    fn recurse(&mut self) -> Error {
        self.recursion = true;
        if let Some(&choice) = self.choices.last() {
            self.deferred.insert(choice);
        }
        Error::msg("recursive type")
    }

    /// Cuts the position `key` if a recursion error passed through it.
    fn maybe_cut(&mut self, key: Vec<Step>, inner: &Format) {
        if self.recursion {
            self.recursion = false;
            self.cut.insert(key, inner.clone());
            self.progress = true;
        }
    }

    fn update(&mut self, name: &'static str, shape: Shape) -> Result<()> {
        match self.registry.get_mut(name) {
            None => {
                self.registry.insert(name, Container::Shape(shape));
                self.progress = true;
            }
            Some(Container::Shape(s)) => {
                let before = s.clone();
                merge_shape(s, shape, name)?;
                if *s != before {
                    self.progress = true;
                }
            }
            Some(Container::Enum(_)) => return Err(conflict(name)),
        }
        Ok(())
    }
}

struct Tracer<'a> {
    st: &'a mut State,
    out: &'a mut Format,
}

macro_rules! prim {
    ($($method:ident => $ty:expr, $visit:ident($($val:expr)?);)*) => {$(
        fn $method<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
            *self.out = Format::Prim($ty);
            visitor.$visit($($val)?)
        }
    )*};
}

impl<'de, 'a> de::Deserializer<'de> for Tracer<'a> {
    type Error = Error;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        *self.out = Format::Any;
        visitor.visit_unit()
    }

    prim! {
        deserialize_bool => Type::BOOL, visit_bool(false);
        deserialize_i8 => Type::INT8, visit_i8(1);
        deserialize_i16 => Type::INT16, visit_i16(1);
        deserialize_i32 => Type::INT32, visit_i32(1);
        deserialize_i64 => Type::INT64, visit_i64(1);
        deserialize_u8 => Type::UINT8, visit_u8(1);
        deserialize_u16 => Type::UINT16, visit_u16(1);
        deserialize_u32 => Type::UINT32, visit_u32(1);
        deserialize_u64 => Type::UINT64, visit_u64(1);
        deserialize_f32 => Type::FLOAT32, visit_f32(1.0);
        deserialize_f64 => Type::FLOAT64, visit_f64(1.0);
        deserialize_char => Type::STRING, visit_char('a');
        deserialize_str => Type::STRING, visit_borrowed_str("");
        deserialize_string => Type::STRING, visit_string(String::new());
        deserialize_bytes => Type::BYTES, visit_borrowed_bytes(&[]);
        deserialize_byte_buf => Type::BYTES, visit_byte_buf(Vec::new());
        deserialize_unit => Type::NULL, visit_unit();
        deserialize_identifier => Type::STRING, visit_borrowed_str("");
    }

    fn deserialize_ignored_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        *self.out = Format::Any;
        visitor.visit_unit()
    }

    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        let key = self.st.key();
        if let Some(f) = self.st.cut.get(&key) {
            *self.out = Format::Option(Box::new(f.clone()));
            return visitor.visit_none();
        }
        let mut inner = Format::Unknown;
        self.st.pos.push(Step::Inner(0));
        let r = visitor.visit_some(Tracer {
            st: self.st,
            out: &mut inner,
        });
        self.st.pos.pop();
        if r.is_err() {
            self.st.maybe_cut(key, &inner);
        }
        *self.out = Format::Option(Box::new(inner));
        r
    }

    fn deserialize_unit_struct<V: Visitor<'de>>(
        self,
        name: &'static str,
        visitor: V,
    ) -> Result<V::Value> {
        self.st.update(name, Shape::Unit)?;
        *self.out = Format::Named(name);
        visitor.visit_unit()
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        name: &'static str,
        visitor: V,
    ) -> Result<V::Value> {
        let magic = match name {
            TIMESTAMP_NAME => Some(Type::TIMESTAMP),
            DURATION_NAME => Some(Type::DURATION),
            _ => None,
        };
        if let Some(t) = magic {
            *self.out = Format::Prim(t);
            let mut scratch = Format::Unknown;
            return visitor.visit_newtype_struct(Tracer {
                st: self.st,
                out: &mut scratch,
            });
        }
        *self.out = Format::Named(name);
        if self.st.in_progress.contains(&name) {
            return Err(self.st.recurse());
        }
        self.st.in_progress.push(name);
        let mut inner = Format::Unknown;
        self.st.pos.push(Step::Slot(name, 0, 0));
        let r = visitor.visit_newtype_struct(Tracer {
            st: self.st,
            out: &mut inner,
        });
        self.st.pos.pop();
        self.st.in_progress.pop();
        self.st.update(name, Shape::Newtype(inner))?;
        r
    }

    fn deserialize_seq<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        let key = self.st.key();
        if let Some(f) = self.st.cut.get(&key) {
            *self.out = Format::Seq(Box::new(f.clone()));
            return visitor.visit_seq(&mut SeqTracer {
                st: self.st,
                slots: Vec::new(),
                next: 0,
                kind: SeqKind::Seq,
            });
        }
        let mut slots = vec![Format::Unknown];
        let r = {
            let seq = SeqTracer {
                st: self.st,
                slots: std::mem::take(&mut slots),
                next: 0,
                kind: SeqKind::Seq,
            };
            visit_seq_collect(visitor, seq, &mut slots)
        };
        let inner = slots.pop().unwrap_or(Format::Unknown);
        if r.is_err() {
            self.st.maybe_cut(key, &inner);
        }
        *self.out = Format::Seq(Box::new(inner));
        r
    }

    fn deserialize_tuple<V: Visitor<'de>>(self, len: usize, visitor: V) -> Result<V::Value> {
        let mut slots = vec![Format::Unknown; len];
        let seq = SeqTracer {
            st: self.st,
            slots: std::mem::take(&mut slots),
            next: 0,
            kind: SeqKind::Tuple,
        };
        let r = visit_seq_collect(visitor, seq, &mut slots);
        *self.out = Format::Tuple(slots);
        r
    }

    fn deserialize_tuple_struct<V: Visitor<'de>>(
        self,
        name: &'static str,
        len: usize,
        visitor: V,
    ) -> Result<V::Value> {
        *self.out = Format::Named(name);
        if self.st.in_progress.contains(&name) {
            return Err(self.st.recurse());
        }
        self.st.in_progress.push(name);
        let mut slots = vec![Format::Unknown; len];
        let seq = SeqTracer {
            st: self.st,
            slots: std::mem::take(&mut slots),
            next: 0,
            kind: SeqKind::Slots(name, 0),
        };
        let r = visit_seq_collect(visitor, seq, &mut slots);
        self.st.in_progress.pop();
        self.st.update(name, Shape::Tuple(slots))?;
        r
    }

    fn deserialize_map<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        let key = self.st.key();
        if let Some(Format::Tuple(kv)) = self.st.cut.get(&key) {
            *self.out = Format::Map(Box::new(kv[0].clone()), Box::new(kv[1].clone()));
            return visitor.visit_map(MapTracer {
                st: self.st,
                key: None,
                val: None,
                done: true,
            });
        }
        let mut k = Format::Unknown;
        let mut v = Format::Unknown;
        let r = visitor.visit_map(MapTracer {
            st: self.st,
            key: Some(&mut k),
            val: Some(&mut v),
            done: false,
        });
        if r.is_err() {
            self.st
                .maybe_cut(key, &Format::Tuple(vec![k.clone(), v.clone()]));
        }
        *self.out = Format::Map(Box::new(k), Box::new(v));
        r
    }

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        name: &'static str,
        fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value> {
        *self.out = Format::Named(name);
        if self.st.in_progress.contains(&name) {
            return Err(self.st.recurse());
        }
        self.st.in_progress.push(name);
        let mut slots = vec![Format::Unknown; fields.len()];
        let map = StructTracer {
            st: self.st,
            fields,
            slots: std::mem::take(&mut slots),
            next: 0,
            anchor: (name, 0),
        };
        let r = visit_struct_collect(visitor, map, &mut slots);
        self.st.in_progress.pop();
        self.st.update(
            name,
            Shape::Struct(fields.iter().copied().zip(slots).collect()),
        )?;
        r
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        name: &'static str,
        variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value> {
        // std::net::IpAddr deserializes as an enum of octets in formats that
        // aren't human readable.
        if name == "IpAddr" && variants == ["V4", "V6"] {
            *self.out = Format::Prim(Type::IP);
            let mut scratch = State::default();
            return visitor.visit_enum(EnumTracer {
                st: &mut scratch,
                name,
                index: 0,
                shape: &mut None,
            });
        }
        *self.out = Format::Named(name);
        let entry = self
            .st
            .registry
            .entry(name)
            .or_insert_with(|| Container::Enum(variants.iter().map(|v| (*v, None)).collect()));
        let Container::Enum(vs) = entry else {
            return Err(conflict(name));
        };
        if vs.len() != variants.len() || vs.iter().zip(variants).any(|(v, w)| v.0 != *w) {
            return Err(conflict(name));
        }
        let done: Vec<bool> = vs
            .iter()
            .map(|(_, s)| s.as_ref().is_some_and(shape_known))
            .collect();
        let index = if self.st.in_progress.contains(&name) {
            // Reentering the enum: choose a variant that's been traced so
            // the recursion ends.
            match done.iter().position(|&d| d) {
                Some(i) => i,
                None => return Err(self.st.recurse()),
            }
        } else {
            (0..done.len())
                .find(|&i| !done[i] && !self.st.deferred.contains(&(name, i)))
                .or_else(|| done.iter().position(|&d| !d))
                .unwrap_or(0)
        };
        if variants.is_empty() {
            return Err(Error::msg(format!("enum {name} has no variants")));
        }
        self.st.in_progress.push(name);
        self.st.choices.push((name, index));
        let mut shape = None;
        let r = visitor.visit_enum(EnumTracer {
            st: self.st,
            name,
            index,
            shape: &mut shape,
        });
        self.st.choices.pop();
        self.st.in_progress.pop();
        if let Some(shape) = shape {
            let Some(Container::Enum(vs)) = self.st.registry.get_mut(name) else {
                unreachable!()
            };
            match &mut vs[index].1 {
                Some(s) => {
                    let before = s.clone();
                    merge_shape(s, shape, name)?;
                    if *s != before {
                        self.st.progress = true;
                    }
                }
                s @ None => {
                    *s = Some(shape);
                    self.st.progress = true;
                }
            }
        }
        r
    }

    fn is_human_readable(&self) -> bool {
        false
    }
}

/// Runs `visitor` on `seq` and copies the traced slots into `slots`.
fn visit_seq_collect<'de, V: Visitor<'de>>(
    visitor: V,
    mut seq: SeqTracer<'_>,
    slots: &mut Vec<Format>,
) -> Result<V::Value> {
    let r = visitor.visit_seq(&mut seq);
    *slots = std::mem::take(&mut seq.slots);
    r
}

fn visit_struct_collect<'de, V: Visitor<'de>>(
    visitor: V,
    mut map: StructTracer<'_>,
    slots: &mut Vec<Format>,
) -> Result<V::Value> {
    let r = visitor.visit_map(&mut map);
    *slots = std::mem::take(&mut map.slots);
    r
}

#[derive(Clone, Copy)]
enum SeqKind {
    Seq,
    Tuple,
    Slots(&'static str, u32),
}

struct SeqTracer<'a> {
    st: &'a mut State,
    slots: Vec<Format>,
    next: usize,
    kind: SeqKind,
}

impl<'de> de::SeqAccess<'de> for SeqTracer<'_> {
    type Error = Error;

    fn next_element_seed<T: DeserializeSeed<'de>>(&mut self, seed: T) -> Result<Option<T::Value>> {
        if self.next >= self.slots.len() {
            return Ok(None);
        }
        let i = self.next;
        self.next += 1;
        let step = match self.kind {
            SeqKind::Slots(name, variant) => Step::Slot(name, variant, i as u32),
            SeqKind::Seq => Step::Inner(1),
            SeqKind::Tuple => Step::Inner(4 + i as u32),
        };
        self.st.pos.push(step);
        let mut slot = std::mem::replace(&mut self.slots[i], Format::Unknown);
        let r = seed.deserialize(Tracer {
            st: self.st,
            out: &mut slot,
        });
        self.slots[i] = slot;
        self.st.pos.pop();
        r.map(Some)
    }

    fn size_hint(&self) -> Option<usize> {
        Some(self.slots.len() - self.next)
    }
}

struct StructTracer<'a> {
    st: &'a mut State,
    fields: &'static [&'static str],
    slots: Vec<Format>,
    next: usize,
    anchor: (&'static str, u32),
}

impl<'de> de::MapAccess<'de> for StructTracer<'_> {
    type Error = Error;

    fn next_key_seed<K: DeserializeSeed<'de>>(&mut self, seed: K) -> Result<Option<K::Value>> {
        if self.next >= self.fields.len() {
            return Ok(None);
        }
        seed.deserialize(BorrowedStrDeserializer::<Error>::new(
            self.fields[self.next],
        ))
        .map(Some)
    }

    fn next_value_seed<V: DeserializeSeed<'de>>(&mut self, seed: V) -> Result<V::Value> {
        let i = self.next;
        self.next += 1;
        self.st
            .pos
            .push(Step::Slot(self.anchor.0, self.anchor.1, i as u32));
        let mut slot = std::mem::replace(&mut self.slots[i], Format::Unknown);
        let r = seed.deserialize(Tracer {
            st: self.st,
            out: &mut slot,
        });
        self.slots[i] = slot;
        self.st.pos.pop();
        r
    }
}

struct MapTracer<'a, 'b> {
    st: &'a mut State,
    key: Option<&'b mut Format>,
    val: Option<&'b mut Format>,
    done: bool,
}

impl<'de> de::MapAccess<'de> for MapTracer<'_, '_> {
    type Error = Error;

    fn next_key_seed<K: DeserializeSeed<'de>>(&mut self, seed: K) -> Result<Option<K::Value>> {
        if self.done {
            return Ok(None);
        }
        self.done = true;
        let out = self.key.as_deref_mut().expect("map key slot");
        self.st.pos.push(Step::Inner(2));
        let r = seed.deserialize(Tracer { st: self.st, out });
        self.st.pos.pop();
        r.map(Some)
    }

    fn next_value_seed<V: DeserializeSeed<'de>>(&mut self, seed: V) -> Result<V::Value> {
        let out = self.val.as_deref_mut().expect("map value slot");
        self.st.pos.push(Step::Inner(3));
        let r = seed.deserialize(Tracer { st: self.st, out });
        self.st.pos.pop();
        r
    }
}

struct EnumTracer<'a, 'b> {
    st: &'a mut State,
    name: &'static str,
    index: usize,
    shape: &'b mut Option<Shape>,
}

impl<'de, 'a, 'b> de::EnumAccess<'de> for EnumTracer<'a, 'b> {
    type Error = Error;
    type Variant = Self;

    fn variant_seed<V: DeserializeSeed<'de>>(self, seed: V) -> Result<(V::Value, Self)> {
        let v = seed.deserialize(U32Deserializer::<Error>::new(self.index as u32))?;
        Ok((v, self))
    }
}

impl<'de, 'a, 'b> de::VariantAccess<'de> for EnumTracer<'a, 'b> {
    type Error = Error;

    fn unit_variant(self) -> Result<()> {
        *self.shape = Some(Shape::Unit);
        Ok(())
    }

    fn newtype_variant_seed<T: DeserializeSeed<'de>>(self, seed: T) -> Result<T::Value> {
        let mut inner = Format::Unknown;
        self.st
            .pos
            .push(Step::Slot(self.name, self.index as u32, 0));
        let r = seed.deserialize(Tracer {
            st: self.st,
            out: &mut inner,
        });
        self.st.pos.pop();
        *self.shape = Some(Shape::Newtype(inner));
        r
    }

    fn tuple_variant<V: Visitor<'de>>(self, len: usize, visitor: V) -> Result<V::Value> {
        let mut slots = vec![Format::Unknown; len];
        let seq = SeqTracer {
            st: self.st,
            slots: std::mem::take(&mut slots),
            next: 0,
            kind: SeqKind::Slots(self.name, self.index as u32),
        };
        let r = visit_seq_collect(visitor, seq, &mut slots);
        *self.shape = Some(Shape::Tuple(slots));
        r
    }

    fn struct_variant<V: Visitor<'de>>(
        self,
        fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value> {
        let mut slots = vec![Format::Unknown; fields.len()];
        let map = StructTracer {
            st: self.st,
            fields,
            slots: std::mem::take(&mut slots),
            next: 0,
            anchor: (self.name, self.index as u32),
        };
        let r = visit_struct_collect(visitor, map, &mut slots);
        *self.shape = Some(Shape::Struct(fields.iter().copied().zip(slots).collect()));
        r
    }
}

/// Returns the static type of `T` as [`type_of`] does, caching it, or
/// `None` if `T` can't be traced.  A type name referring to two different
/// Rust types is an error.
pub(crate) fn cached_type_of<T: de::DeserializeOwned>() -> Result<Option<Type>> {
    use std::sync::{Mutex, OnceLock};
    static CACHE: OnceLock<Mutex<HashMap<&'static str, Option<Type>>>> = OnceLock::new();
    let key = std::any::type_name::<T>();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(t) = cache.lock().unwrap_or_else(|e| e.into_inner()).get(key) {
        return Ok(t.clone());
    }
    let t = match type_of::<T>() {
        Ok(t) => Some(t),
        Err(err @ Error::Redefined(_)) => return Err(err),
        Err(_) => None,
    };
    cache
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(key, t.clone());
    Ok(t)
}

/// Bounds the number of tracing passes.
const MAX_PASSES: usize = 1000;

/// Returns the static super-structured type of `T` (see the [module
/// documentation](self) for how Rust types map to super-structured types).
///
/// It fails if `T`'s `Deserialize` impl can't be traced (e.g., it rejects
/// the placeholder values the tracer supplies) or if a type name refers to
/// two different Rust types (e.g., the instances of a generic struct).
pub fn type_of<T: de::DeserializeOwned>() -> Result<Type> {
    let mut st = State::default();
    let mut root = Format::Unknown;
    for _ in 0..MAX_PASSES {
        st.progress = false;
        st.recursion = false;
        st.pos.clear();
        st.in_progress.clear();
        st.choices.clear();
        let mut out = Format::Unknown;
        let r = T::deserialize(Tracer {
            st: &mut st,
            out: &mut out,
        });
        if let Err(err @ Error::Redefined(_)) = r {
            return Err(err);
        }
        merge(&mut root, out, "(root)")?;
        if r.is_ok() && complete(&st) {
            return build(&root, &st.registry);
        }
        if !st.progress {
            return Err(match r {
                Err(err) => Error::msg(format!("can't trace the type: {err}")),
                Ok(_) => Error::msg("can't trace all of the type's enum variants"),
            });
        }
    }
    Err(Error::msg(
        "can't trace the type in a bounded number of passes",
    ))
}

fn complete(st: &State) -> bool {
    st.registry.values().all(|c| match c {
        Container::Shape(s) => shape_known(s),
        Container::Enum(vs) => vs.iter().all(|(_, s)| s.as_ref().is_some_and(shape_known)),
    })
}

fn build(root: &Format, registry: &HashMap<&'static str, Container>) -> Result<Type> {
    let mut b = Builder {
        registry,
        named: HashMap::new(),
    };
    b.format(root)
}

struct Builder<'a> {
    registry: &'a HashMap<&'static str, Container>,
    named: HashMap<String, Type>,
}

impl Builder<'_> {
    fn format(&mut self, f: &Format) -> Result<Type> {
        Ok(match f {
            Format::Unknown | Format::Any => Type::fusion(Type::ALL),
            Format::Prim(t) => t.clone(),
            Format::Option(inner) => {
                let t = self.format(inner)?;
                if t == Type::fusion(Type::ALL) || t == Type::NULL {
                    t
                } else {
                    Type::union([t, Type::NULL])?
                }
            }
            Format::Seq(inner) => Type::array(self.format(inner)?),
            Format::Map(k, v) => {
                let k = self.format(k)?;
                Type::map(k, self.format(v)?)
            }
            Format::Tuple(fs) => self.tuple(fs)?,
            Format::Named(name) => self.container(name)?,
        })
    }

    fn tuple(&mut self, fs: &[Format]) -> Result<Type> {
        let fields = fs
            .iter()
            .enumerate()
            .map(|(i, f)| Ok(Field::new(format!("column{}", i + 1), self.format(f)?)))
            .collect::<Result<Vec<_>>>()?;
        Ok(Type::Record(fields.into()))
    }

    fn shape(&mut self, s: Option<&Shape>) -> Result<Type> {
        match s {
            None | Some(Shape::Unit) => Ok(Type::NULL),
            Some(Shape::Newtype(f)) => self.format(f),
            Some(Shape::Tuple(fs)) => self.tuple(fs),
            Some(Shape::Struct(fs)) => {
                let fields = fs
                    .iter()
                    .map(|(n, f)| Ok(Field::new(*n, self.format(f)?)))
                    .collect::<Result<Vec<_>>>()?;
                Type::record(fields)
            }
        }
    }

    /// Returns the named type `name`, declaring it before building its
    /// underlying type, which may refer to it.
    fn named(
        &mut self,
        name: String,
        build: impl FnOnce(&mut Self) -> Result<Type>,
    ) -> Result<Type> {
        if let Some(t) = self.named.get(&name) {
            return Ok(t.clone());
        }
        crate::types::check_type_name(&name)?;
        let named = NamedType::declare(name.clone());
        let t = Type::Named(named.clone());
        self.named.insert(name, t.clone());
        named.bind(build(self)?);
        Ok(t)
    }

    fn container(&mut self, name: &'static str) -> Result<Type> {
        let c = self
            .registry
            .get(name)
            .ok_or_else(|| Error::msg(format!("type {name} wasn't traced")))?;
        match c {
            Container::Shape(s) => {
                let s = s.clone();
                self.named(name.to_string(), |b| b.shape(Some(&s)))
            }
            Container::Enum(vs) => {
                let vs = vs.clone();
                if vs.iter().all(|(_, s)| matches!(s, Some(Shape::Unit))) {
                    let symbols = vs.iter().map(|(v, _)| v.to_string()).collect();
                    return self.named(name.to_string(), |_| Ok(Type::enumeration(symbols)));
                }
                self.named(name.to_string(), |b| {
                    let mut types = Vec::with_capacity(vs.len());
                    for (v, s) in &vs {
                        let s = s.clone();
                        types.push(b.named(format!("{name}_{v}"), |b| b.shape(s.as_ref()))?);
                    }
                    Type::union(types)
                })
            }
        }
    }
}

impl de::Error for Error {
    fn custom<T: Display>(msg: T) -> Self {
        Error::Message(msg.to_string())
    }
}
