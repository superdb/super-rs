//! The super-structured type system.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, OnceLock};

use crate::error::{Error, Result};

/// A primitive type, whose discriminant is its BSUP type ID.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u8)]
#[allow(missing_docs)]
pub enum Primitive {
    Uint8 = 0,
    Uint16 = 1,
    Uint32 = 2,
    Uint64 = 3,
    Uint128 = 4,
    Int8 = 5,
    Int16 = 6,
    Int32 = 7,
    Int64 = 8,
    Int128 = 9,
    Duration = 10,
    Timestamp0 = 11,
    Timestamp3 = 12,
    Timestamp6 = 13,
    Timestamp = 14,
    TimestampNtz0 = 15,
    TimestampNtz3 = 16,
    TimestampNtz6 = 17,
    TimestampNtz = 18,
    Date = 19,
    Time0 = 20,
    Time3 = 21,
    Time6 = 22,
    Time = 23,
    Float16 = 24,
    Float32 = 25,
    Float64 = 26,
    Bool = 27,
    Bytes = 28,
    String = 29,
    Ip = 30,
    Net = 31,
    Interval = 32,
    Uuid = 33,
    Geometry = 34,
    Geography = 35,
    Type = 36,
    Null = 37,
    Missing = 38,
    None = 39,
    All = 40,
    Any = 41,
}

/// The lowest type ID of a complex type in a typedefs table.
pub const FIRST_COMPLEX_ID: u32 = 48;

const PRIMITIVES: [Primitive; 42] = {
    use Primitive::*;
    [
        Uint8,
        Uint16,
        Uint32,
        Uint64,
        Uint128,
        Int8,
        Int16,
        Int32,
        Int64,
        Int128,
        Duration,
        Timestamp0,
        Timestamp3,
        Timestamp6,
        Timestamp,
        TimestampNtz0,
        TimestampNtz3,
        TimestampNtz6,
        TimestampNtz,
        Date,
        Time0,
        Time3,
        Time6,
        Time,
        Float16,
        Float32,
        Float64,
        Bool,
        Bytes,
        String,
        Ip,
        Net,
        Interval,
        Uuid,
        Geometry,
        Geography,
        Type,
        Null,
        Missing,
        None,
        All,
        Any,
    ]
};

impl Primitive {
    /// Returns the primitive type with BSUP type ID `id`.
    pub fn from_id(id: u32) -> Option<Primitive> {
        PRIMITIVES.get(id as usize).copied()
    }

    /// Returns the BSUP type ID of the type.
    pub fn id(self) -> u32 {
        self as u32
    }

    /// Returns the SUP name of the type.
    pub fn name(self) -> &'static str {
        use Primitive::*;
        match self {
            Uint8 => "uint8",
            Uint16 => "uint16",
            Uint32 => "uint32",
            Uint64 => "uint64",
            Uint128 => "uint128",
            Int8 => "int8",
            Int16 => "int16",
            Int32 => "int32",
            Int64 => "int64",
            Int128 => "int128",
            Duration => "duration",
            Timestamp0 => "timestamp(0)",
            Timestamp3 => "timestamp(3)",
            Timestamp6 => "timestamp(6)",
            Timestamp => "timestamp",
            TimestampNtz0 => "timestamp_ntz(0)",
            TimestampNtz3 => "timestamp_ntz(3)",
            TimestampNtz6 => "timestamp_ntz(6)",
            TimestampNtz => "timestamp_ntz",
            Date => "date",
            Time0 => "time(0)",
            Time3 => "time(3)",
            Time6 => "time(6)",
            Time => "time",
            Float16 => "float16",
            Float32 => "float32",
            Float64 => "float64",
            Bool => "bool",
            Bytes => "bytes",
            String => "string",
            Ip => "ip",
            Net => "net",
            Interval => "interval",
            Uuid => "uuid",
            Geometry => "geometry",
            Geography => "geography",
            Type => "type",
            Null => "null",
            Missing => "missing",
            None => "none",
            All => "all",
            Any => "any",
        }
    }

    /// Returns the primitive type whose SUP name is `name`.
    pub fn from_name(name: &str) -> Option<Primitive> {
        PRIMITIVES.iter().copied().find(|p| p.name() == name)
    }

    /// Returns true if values of the type have a defined body (i.e., the
    /// type isn't reserved by the BSUP specification).
    pub fn has_body(self) -> bool {
        use Primitive::*;
        !matches!(
            self,
            Uint128
                | Int128
                | Timestamp0
                | Timestamp3
                | Timestamp6
                | TimestampNtz0
                | TimestampNtz3
                | TimestampNtz6
                | TimestampNtz
                | Date
                | Time0
                | Time3
                | Time6
                | Time
                | Interval
                | Geometry
                | Geography
                | Any
        )
    }
}

/// A field of a record type.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Field {
    /// The name of the field.
    pub name: String,
    /// The type of the field.
    pub ty: Type,
}

impl Field {
    /// Returns a field with `name` and type `ty`.
    pub fn new(name: impl Into<String>, ty: Type) -> Field {
        Field {
            name: name.into(),
            ty,
        }
    }
}

/// A named type, which binds a name to a type.  The type may refer to the
/// named type (i.e., it may be recursive), which is why it's set once after
/// the named type is created.
pub struct NamedType {
    name: String,
    inner: OnceLock<Type>,
}

impl NamedType {
    /// Returns the name of the type.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the underlying type.
    ///
    /// # Panics
    ///
    /// Panics if the named type was declared but never bound, which can't
    /// happen for a type returned by this crate.
    pub fn inner(&self) -> &Type {
        self.inner.get().expect("named type is not bound")
    }

    pub(crate) fn declare(name: impl Into<String>) -> Arc<NamedType> {
        Arc::new(NamedType {
            name: name.into(),
            inner: OnceLock::new(),
        })
    }

    pub(crate) fn bind(&self, inner: Type) {
        let _ = self.inner.set(inner);
    }

    pub(crate) fn is_bound(&self) -> bool {
        self.inner.get().is_some()
    }
}

impl PartialEq for NamedType {
    fn eq(&self, other: &NamedType) -> bool {
        std::ptr::eq(self, other)
            || (self.name == other.name && self.inner.get() == other.inner.get())
    }
}

impl fmt::Debug for NamedType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Named({:?})", self.name)
    }
}

/// A super-structured type.
///
/// Types compare structurally, so two types built separately from the same
/// parts are equal, which mirrors the type IDs of a SuperDB type context.
/// Cloning a type is cheap.
#[derive(Clone)]
pub enum Type {
    /// A primitive type.
    Primitive(Primitive),
    /// A record type: an ordered list of fields with unique names.
    Record(Arc<[Field]>),
    /// An array type with its element type.
    Array(Arc<Type>),
    /// A set type with its element type.
    Set(Arc<Type>),
    /// A map type with its key type and value type.
    Map(Arc<(Type, Type)>),
    /// A union type with its member types in canonical order (see
    /// [`compare_types`]).
    Union(Arc<[Type]>),
    /// An enum type with its symbols.
    Enum(Arc<[String]>),
    /// An error type with the type of its values.
    Error(Arc<Type>),
    /// A named type.
    Named(Arc<NamedType>),
    /// A fusion type with its supertype.
    Fusion(Arc<Type>),
    /// An option type with the type of its values when they're present.
    Option(Arc<Type>),
    /// A decimal type with its precision and scale.  (Decimal values are
    /// reserved and not yet defined.)
    Decimal(u32, u32),
}

/// The kind of a type, which orders types in canonical order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[allow(missing_docs)]
pub enum Kind {
    Primitive,
    Record,
    Array,
    Set,
    Map,
    Union,
    Enum,
    Error,
    Fusion,
    Option,
    Decimal,
}

#[allow(missing_docs)]
impl Type {
    pub const UINT8: Type = Type::Primitive(Primitive::Uint8);
    pub const UINT16: Type = Type::Primitive(Primitive::Uint16);
    pub const UINT32: Type = Type::Primitive(Primitive::Uint32);
    pub const UINT64: Type = Type::Primitive(Primitive::Uint64);
    pub const INT8: Type = Type::Primitive(Primitive::Int8);
    pub const INT16: Type = Type::Primitive(Primitive::Int16);
    pub const INT32: Type = Type::Primitive(Primitive::Int32);
    pub const INT64: Type = Type::Primitive(Primitive::Int64);
    pub const DURATION: Type = Type::Primitive(Primitive::Duration);
    pub const TIMESTAMP: Type = Type::Primitive(Primitive::Timestamp);
    pub const FLOAT16: Type = Type::Primitive(Primitive::Float16);
    pub const FLOAT32: Type = Type::Primitive(Primitive::Float32);
    pub const FLOAT64: Type = Type::Primitive(Primitive::Float64);
    pub const BOOL: Type = Type::Primitive(Primitive::Bool);
    pub const BYTES: Type = Type::Primitive(Primitive::Bytes);
    pub const STRING: Type = Type::Primitive(Primitive::String);
    pub const IP: Type = Type::Primitive(Primitive::Ip);
    pub const NET: Type = Type::Primitive(Primitive::Net);
    pub const UUID: Type = Type::Primitive(Primitive::Uuid);
    pub const TYPE: Type = Type::Primitive(Primitive::Type);
    pub const NULL: Type = Type::Primitive(Primitive::Null);
    pub const MISSING: Type = Type::Primitive(Primitive::Missing);
    pub const NONE: Type = Type::Primitive(Primitive::None);
    pub const ALL: Type = Type::Primitive(Primitive::All);
    pub const ANY: Type = Type::Primitive(Primitive::Any);
}

impl Type {
    /// Returns a record type with `fields`, which must have unique names.
    pub fn record(fields: Vec<Field>) -> Result<Type> {
        for (i, f) in fields.iter().enumerate() {
            if fields[..i].iter().any(|g| g.name == f.name) {
                return Err(Error::msg(format!(
                    "duplicate field {:?} in record type",
                    f.name
                )));
            }
        }
        Ok(Type::Record(fields.into()))
    }

    /// Returns an array type.
    pub fn array(elem: Type) -> Type {
        Type::Array(Arc::new(elem))
    }

    /// Returns a set type.
    pub fn set(elem: Type) -> Type {
        Type::Set(Arc::new(elem))
    }

    /// Returns a map type.
    pub fn map(key: Type, val: Type) -> Type {
        Type::Map(Arc::new((key, val)))
    }

    /// Returns the union of `types`, which is flattened (a member that's a
    /// union contributes its members), deduplicated, and sorted in canonical
    /// order.  If a single type remains, that type is returned.
    pub fn union(types: impl IntoIterator<Item = Type>) -> Result<Type> {
        let types = flatten(types);
        match types.len() {
            0 => Err(Error::msg("union of no types")),
            1 => Ok(types.into_iter().next().unwrap()),
            _ => Ok(Type::Union(types.into())),
        }
    }

    /// Returns an enum type with `symbols`.
    pub fn enumeration(symbols: Vec<String>) -> Type {
        Type::Enum(symbols.into())
    }

    /// Returns an error type.
    pub fn error(inner: Type) -> Type {
        Type::Error(Arc::new(inner))
    }

    /// Returns a named type binding `name` to `inner`.
    pub fn named(name: impl Into<String>, inner: Type) -> Result<Type> {
        let name = name.into();
        check_type_name(&name)?;
        let named = NamedType::declare(name);
        named.bind(inner);
        Ok(Type::Named(named))
    }

    /// Returns a fusion type with supertype `inner`.
    pub fn fusion(inner: Type) -> Type {
        Type::Fusion(Arc::new(inner))
    }

    /// Returns an option type.  The inner type may not be an option,
    /// `missing`, or `none`.
    pub fn option(inner: Type) -> Result<Type> {
        match inner {
            Type::Option(_) | Type::Primitive(Primitive::Missing | Primitive::None) => {
                Err(Error::msg(format!("bad option type: option({inner})")))
            }
            _ => Ok(Type::Option(Arc::new(inner))),
        }
    }

    /// Returns the kind of the type, or the kind of the underlying type of a
    /// named type.
    pub fn kind(&self) -> Kind {
        match self {
            Type::Primitive(_) => Kind::Primitive,
            Type::Record(_) => Kind::Record,
            Type::Array(_) => Kind::Array,
            Type::Set(_) => Kind::Set,
            Type::Map(_) => Kind::Map,
            Type::Union(_) => Kind::Union,
            Type::Enum(_) => Kind::Enum,
            Type::Error(_) => Kind::Error,
            Type::Named(n) => n.inner().kind(),
            Type::Fusion(_) => Kind::Fusion,
            Type::Option(_) => Kind::Option,
            Type::Decimal(..) => Kind::Decimal,
        }
    }

    /// Returns the underlying type of a named type (following named types
    /// that name named types) or the type itself.
    pub fn under(&self) -> &Type {
        let mut t = self;
        while let Type::Named(n) = t {
            t = n.inner();
        }
        t
    }

    /// Returns the primitive type if the type is primitive.
    pub fn primitive(&self) -> Option<Primitive> {
        match self {
            Type::Primitive(p) => Some(*p),
            _ => None,
        }
    }

    /// Returns the fields of a record type.
    pub fn fields(&self) -> Option<&[Field]> {
        match self {
            Type::Record(fields) => Some(fields),
            _ => None,
        }
    }

    /// Returns true if the type is a named type.
    pub fn is_named(&self) -> bool {
        matches!(self, Type::Named(_))
    }

    /// Returns the tag of `member` in a union type.
    pub fn union_tag(&self, member: &Type) -> Option<usize> {
        match self {
            Type::Union(types) => types.iter().position(|t| t == member),
            _ => None,
        }
    }
}

pub(crate) fn check_type_name(name: &str) -> Result<()> {
    if Primitive::from_name(name).is_some() {
        return Err(Error::msg(format!(
            "named type collides with primitive type: {name}"
        )));
    }
    Ok(())
}

/// Flattens, deduplicates, and sorts `types` in canonical order.
pub(crate) fn flatten(types: impl IntoIterator<Item = Type>) -> Vec<Type> {
    let mut out = Vec::new();
    fn add(out: &mut Vec<Type>, t: Type) {
        if let Type::Union(members) = &t {
            for m in members.iter() {
                add(out, m.clone());
            }
        } else {
            out.push(t);
        }
    }
    for t in types {
        add(&mut out, t);
    }
    unique_types(out)
}

/// Sorts `types` in canonical order and removes duplicates.
pub(crate) fn unique_types(mut types: Vec<Type>) -> Vec<Type> {
    types.sort_by(compare_types);
    types.dedup();
    types
}

impl PartialEq for Type {
    fn eq(&self, other: &Type) -> bool {
        match (self, other) {
            (Type::Primitive(a), Type::Primitive(b)) => a == b,
            (Type::Record(a), Type::Record(b)) => Arc::ptr_eq(a, b) || a == b,
            (Type::Array(a), Type::Array(b))
            | (Type::Set(a), Type::Set(b))
            | (Type::Error(a), Type::Error(b))
            | (Type::Fusion(a), Type::Fusion(b))
            | (Type::Option(a), Type::Option(b)) => Arc::ptr_eq(a, b) || a == b,
            (Type::Map(a), Type::Map(b)) => Arc::ptr_eq(a, b) || a == b,
            (Type::Union(a), Type::Union(b)) => Arc::ptr_eq(a, b) || a == b,
            (Type::Enum(a), Type::Enum(b)) => Arc::ptr_eq(a, b) || a == b,
            (Type::Named(a), Type::Named(b)) => **a == **b,
            (Type::Decimal(p1, s1), Type::Decimal(p2, s2)) => p1 == p2 && s1 == s2,
            _ => false,
        }
    }
}

impl Eq for Type {}

impl Hash for Type {
    fn hash<H: Hasher>(&self, state: &mut H) {
        std::mem::discriminant(self).hash(state);
        match self {
            Type::Primitive(p) => p.hash(state),
            Type::Record(fields) => fields.hash(state),
            Type::Array(t) | Type::Set(t) | Type::Error(t) | Type::Fusion(t) | Type::Option(t) => {
                t.hash(state)
            }
            Type::Map(kv) => kv.hash(state),
            Type::Union(types) => types.hash(state),
            Type::Enum(symbols) => symbols.hash(state),
            // Hash only the name so recursive types hash finitely.
            Type::Named(n) => n.name.hash(state),
            Type::Decimal(p, s) => (p, s).hash(state),
        }
    }
}

/// Compares types in the canonical order of SuperDB, which orders the
/// members of a union type: named types first (by name), then by kind,
/// then by the type's parts.
pub fn compare_types(a: &Type, b: &Type) -> Ordering {
    if a == b {
        return Ordering::Equal;
    }
    match (a, b) {
        (Type::Named(x), Type::Named(y)) => {
            return x
                .name
                .cmp(&y.name)
                .then_with(|| compare_types(x.inner(), y.inner()));
        }
        (Type::Named(_), _) => return Ordering::Less,
        (_, Type::Named(_)) => return Ordering::Greater,
        _ => {}
    }
    let kind = a.kind().cmp(&b.kind());
    if kind != Ordering::Equal {
        return kind;
    }
    match (a, b) {
        (Type::Primitive(x), Type::Primitive(y)) => x.cmp(y),
        (Type::Record(x), Type::Record(y)) => x
            .len()
            .cmp(&y.len())
            .then_with(|| {
                x.iter()
                    .zip(y.iter())
                    .map(|(f, g)| f.name.cmp(&g.name))
                    .find(|o| o.is_ne())
                    .unwrap_or(Ordering::Equal)
            })
            .then_with(|| {
                x.iter()
                    .zip(y.iter())
                    .map(|(f, g)| compare_types(&f.ty, &g.ty))
                    .find(|o| o.is_ne())
                    .unwrap_or(Ordering::Equal)
            }),
        (Type::Array(x), Type::Array(y))
        | (Type::Set(x), Type::Set(y))
        | (Type::Error(x), Type::Error(y))
        | (Type::Fusion(x), Type::Fusion(y))
        | (Type::Option(x), Type::Option(y)) => compare_types(x, y),
        (Type::Map(x), Type::Map(y)) => {
            compare_types(&x.0, &y.0).then_with(|| compare_types(&x.1, &y.1))
        }
        (Type::Union(x), Type::Union(y)) => x.len().cmp(&y.len()).then_with(|| {
            x.iter()
                .zip(y.iter())
                .map(|(s, t)| compare_types(s, t))
                .find(|o| o.is_ne())
                .unwrap_or(Ordering::Equal)
        }),
        (Type::Enum(x), Type::Enum(y)) => {
            x.len().cmp(&y.len()).then_with(|| x.iter().cmp(y.iter()))
        }
        (Type::Decimal(p1, s1), Type::Decimal(p2, s2)) => p1.cmp(p2).then(s1.cmp(s2)),
        _ => Ordering::Equal,
    }
}

/// Formats a type like its SUP syntax but with the first occurrence of each
/// named type formatted as its definition (`name=type`).
impl fmt::Debug for Type {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut s = String::new();
        format_type(&mut s, self, &mut Some(HashMap::new()));
        f.write_str(&s)
    }
}

/// Formats a type in SUP syntax, where a named type is formatted as its
/// name (see [`Type::declarations`] for its definition).
impl fmt::Display for Type {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut s = String::new();
        format_type(&mut s, self, &mut None);
        f.write_str(&s)
    }
}

impl Type {
    /// Returns the SUP type declarations (`type name=type`) of the named
    /// types in the type, in an order in which each declaration follows the
    /// declarations of the named types it refers to.
    pub fn declarations(&self) -> Vec<String> {
        let mut out = Vec::new();
        let mut seen = std::collections::HashSet::new();
        declare(self, &mut seen, &mut out);
        out
    }
}

pub(crate) fn declare(
    t: &Type,
    seen: &mut std::collections::HashSet<String>,
    out: &mut Vec<String>,
) {
    match t {
        Type::Primitive(_) | Type::Enum(_) | Type::Decimal(..) => {}
        Type::Record(fields) => fields.iter().for_each(|f| declare(&f.ty, seen, out)),
        Type::Array(x) | Type::Set(x) | Type::Error(x) | Type::Fusion(x) | Type::Option(x) => {
            declare(x, seen, out)
        }
        Type::Map(kv) => {
            declare(&kv.0, seen, out);
            declare(&kv.1, seen, out);
        }
        Type::Union(types) => types.iter().for_each(|t| declare(t, seen, out)),
        Type::Named(n) => {
            if seen.insert(n.name.clone()) {
                declare(n.inner(), seen, out);
                let mut s = String::from("type ");
                format_type_name(&mut s, &n.name);
                s.push('=');
                format_type(&mut s, n.inner(), &mut None);
                out.push(s);
            }
        }
    }
}

pub(crate) fn format_type(s: &mut String, t: &Type, defs: &mut Option<HashMap<String, ()>>) {
    match t {
        Type::Primitive(p) => s.push_str(p.name()),
        Type::Record(fields) => {
            s.push('{');
            for (i, field) in fields.iter().enumerate() {
                if i > 0 {
                    s.push(',');
                }
                format_field_name(s, &field.name);
                match &field.ty {
                    Type::Option(inner) => {
                        s.push_str("?:");
                        format_type(s, inner, defs);
                    }
                    t => {
                        s.push(':');
                        format_type(s, t, defs);
                    }
                }
            }
            s.push('}');
        }
        Type::Array(t) => {
            s.push('[');
            format_type(s, t, defs);
            s.push(']');
        }
        Type::Set(t) => {
            s.push_str("set[");
            format_type(s, t, defs);
            s.push(']');
        }
        Type::Map(kv) => {
            s.push_str("map{");
            format_type(s, &kv.0, defs);
            s.push(':');
            format_type(s, &kv.1, defs);
            s.push('}');
        }
        Type::Union(types) => {
            for (i, t) in types.iter().enumerate() {
                if i > 0 {
                    s.push('|');
                }
                let paren = matches!(t, Type::Named(_))
                    && defs
                        .as_ref()
                        .is_some_and(|d| !d.contains_key(named_name(t)));
                if paren {
                    s.push('(');
                }
                format_type(s, t, defs);
                if paren {
                    s.push(')');
                }
            }
        }
        Type::Enum(symbols) => {
            s.push_str("enum(");
            for (i, sym) in symbols.iter().enumerate() {
                if i > 0 {
                    s.push(',');
                }
                format_field_name(s, sym);
            }
            s.push(')');
        }
        Type::Error(t) => {
            s.push_str("error(");
            format_type(s, t, defs);
            s.push(')');
        }
        Type::Named(n) => {
            format_type_name(s, &n.name);
            if let Some(d) = defs {
                if d.insert(n.name.clone(), ()).is_none() {
                    s.push('=');
                    match n.inner.get() {
                        Some(inner) => format_type(s, inner, defs),
                        None => s.push('?'),
                    }
                }
            }
        }
        Type::Fusion(t) => {
            s.push_str("fusion(");
            format_type(s, t, defs);
            s.push(')');
        }
        Type::Option(t) => {
            s.push_str("option(");
            format_type(s, t, defs);
            s.push(')');
        }
        Type::Decimal(p, sc) => s.push_str(&format!("decimal({p},{sc})")),
    }
}

fn named_name(t: &Type) -> &str {
    match t {
        Type::Named(n) => &n.name,
        _ => "",
    }
}

pub(crate) fn is_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_alphabetic() || c == '_' || c == '$' => {}
        _ => return false,
    }
    chars.all(|c| c.is_alphabetic() || c.is_ascii_digit() || c == '_' || c == '$')
}

pub(crate) fn format_field_name(s: &mut String, name: &str) {
    if is_identifier(name) {
        s.push_str(name);
    } else {
        crate::value::quote_string(s, name);
    }
}

pub(crate) fn format_type_name(s: &mut String, name: &str) {
    if is_identifier(name) {
        s.push_str(name);
    } else {
        crate::value::quote_string(s, name);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primitive_ids() {
        for id in 0..42 {
            let p = Primitive::from_id(id).unwrap();
            assert_eq!(p.id(), id);
            assert_eq!(Primitive::from_name(p.name()), Some(p));
        }
        assert!(Primitive::from_id(42).is_none());
    }

    #[test]
    fn union_order() {
        let point = Type::named(
            "Point",
            Type::record(vec![Field::new("x", Type::INT64)]).unwrap(),
        )
        .unwrap();
        let u = Type::union([
            Type::STRING,
            Type::array(Type::INT64),
            point.clone(),
            Type::INT64,
            Type::STRING,
        ])
        .unwrap();
        assert_eq!(format!("{u:?}"), "(Point={x:int64})|int64|string|[int64]");
        assert_eq!(u.to_string(), "Point|int64|string|[int64]");
        assert_eq!(u.declarations(), vec!["type Point={x:int64}"]);
        assert_eq!(u.union_tag(&Type::INT64), Some(1));
        assert_eq!(Type::union([Type::INT64]).unwrap(), Type::INT64);
    }

    #[test]
    fn structural_equality() {
        let a = Type::record(vec![Field::new("a", Type::array(Type::STRING))]).unwrap();
        let b = Type::record(vec![Field::new("a", Type::array(Type::STRING))]).unwrap();
        assert_eq!(a, b);
        assert_eq!(compare_types(&a, &b), Ordering::Equal);
        assert!(Type::named("int64", Type::INT64).is_err());
        assert!(Type::option(Type::NONE).is_err());
    }
}
