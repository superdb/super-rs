//! Super Binary (BSUP) encoding of super-structured data.
//!
//! This crate is the Rust implementation of [BSUP], the binary format of
//! [SuperDB].  It provides
//!
//! * the super-structured type system ([`Type`]) and values ([`Value`]),
//! * a [`bsup::Reader`] of BSUP row and column frames and a [`bsup::Writer`]
//!   of row frames,
//! * serde support: [`to_value`] and [`from_value`] convert between Rust
//!   values and super-structured values, and the reader and writer read and
//!   write Rust values directly, and
//! * the fuser that computes the fused type of a sequence of values
//!   ([`Fuser`]).
//!
//! # Types of Rust values
//!
//! Rust has no runtime reflection, so [`to_value`] discovers the static
//! type of a Rust type by tracing its serde `Deserialize` impl (see
//! [`schema`]).  A struct `Point` becomes the named type `Point={x:...}` and
//! an enum's variants become named types (`Shape_Circle`, `Shape_Rect`, ...)
//! so a value retains its Rust type and deserializes back to it, and SuperDB
//! queries can tell the variants apart (e.g., `where is(<Shape_Circle>)`).
//! An `Option<T>` is the option type `option(T)`, so an `Option` field of a
//! struct is an optional field of its record (`{name?:T}`).
//!
//! ```
//! use serde::{Deserialize, Serialize};
//!
//! #[derive(Serialize, Deserialize, Debug, PartialEq)]
//! enum Shape {
//!     Circle { r: f64 },
//!     Rect { w: f64, h: f64 },
//! }
//!
//! # fn main() -> superdb::Result<()> {
//! let shapes = vec![Shape::Circle { r: 1.0 }, Shape::Rect { w: 2.0, h: 3.0 }];
//! let bsup = superdb::to_vec(&shapes)?;
//! assert_eq!(superdb::from_slice::<Shape>(&bsup)?, shapes);
//! assert_eq!(
//!     superdb::to_value(&shapes[0])?.to_string(),
//!     "{r:1.}::Shape_Circle::Shape"
//! );
//! # Ok(())
//! # }
//! ```
//!
//! [BSUP]: https://superdb.org/docs/formats/bsup
//! [SuperDB]: https://superdb.org

#![warn(missing_docs)]

pub mod bsup;
mod coerce;
pub mod de;
mod error;
mod fuser;
pub mod schema;
pub mod ser;
pub mod sup;
mod typedefs;
mod types;
mod typevalue;
mod value;
mod varint;

pub use error::{Error, Result};
pub use fuser::Fuser;
pub use schema::type_of;
pub use ser::{Duration, Timestamp};
pub use types::{compare_types, Field, Kind, NamedType, Primitive, Type};
pub use typevalue::{decode_type_value, encode_type_value};
pub use value::Value;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// Converts a Rust value to a super-structured value whose type is the
/// static type of `T` (see [`schema`]), with named types for its structs and
/// enums, so every value of `T` has the same type.
///
/// If `T` can't be traced (e.g., its `Deserialize` impl uses
/// `deserialize_any`, as untagged and internally tagged enums do), the type
/// is inferred from the value as [`to_value_untyped`] does.  It fails if a
/// type name refers to two different Rust types ([`Error::Redefined`]) or
/// if the value doesn't conform to `T`'s static type (e.g., a field that
/// isn't an `Option` is skipped when it's serialized).
pub fn to_value<T: Serialize + DeserializeOwned>(value: &T) -> Result<Value> {
    let v = value.serialize(ser::Serializer)?;
    match schema::cached_type_of::<T>()? {
        Some(t) => coerce::coerce(v, &t),
        None => Ok(v),
    }
}

/// Converts a Rust value to a super-structured value whose type is inferred
/// from the value (see [`ser`]): structs are anonymous records, `Some(x)` is
/// the option value `some(x)`, `None` is `none`, and enum variants are
/// represented as they are in JSON.
pub fn to_value_untyped<T: Serialize + ?Sized>(value: &T) -> Result<Value> {
    value.serialize(ser::Serializer)
}

/// Converts a super-structured value to a Rust value (see [`de`]).
pub fn from_value<'de, T: Deserialize<'de>>(value: &'de Value) -> Result<T> {
    T::deserialize(de::Deserializer::new(value))
}

/// Returns the BSUP encoding of `values` (see [`to_value`]).
pub fn to_vec<T: Serialize + DeserializeOwned>(values: &[T]) -> Result<Vec<u8>> {
    let mut w = bsup::Writer::new(Vec::new());
    for v in values {
        w.write(v)?;
    }
    w.finish()
}

/// Decodes the BSUP in `bytes` into Rust values (see [`from_value`]).
pub fn from_slice<T: DeserializeOwned>(bytes: &[u8]) -> Result<Vec<T>> {
    bsup::Reader::new(bytes).deserialize().collect()
}
