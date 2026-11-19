# superdb

Rust support for [SuperDB](https://superdb.org), starting with its binary
format, [Super Binary (BSUP)](https://superdb.org/docs/formats/bsup).

This crate provides

* the super-structured type system (`Type`) and values (`Value`),
* a BSUP `Reader` of row frames and column frames and a `Writer` of row
  frames (BSUP version 0.31),
* serde support, so Rust values can be written to and read from BSUP while
  retaining their types, and
* a SUP formatter (`superdb::sup`) whose output matches `super -s`.

## Rust types as super-structured types

Rust has no runtime reflection, so the crate discovers the static type of a
Rust type by tracing its serde `Deserialize` impl, which describes the type's
shape: a struct's name and fields, all of an enum's variants, and the element
types of options and sequences.  Every value of a Rust type then has the same
super-structured type, with named types for structs and enums:

```rust
#[derive(Serialize, Deserialize)]
struct Thing { name: String, color: Color, shapes: Vec<Shape>, at: Option<Point> }

#[derive(Serialize, Deserialize)]
enum Color { Red, Green, Blue }

#[derive(Serialize, Deserialize)]
enum Shape { Circle(f64), Rect { w: f64, h: f64 }, Empty }
```

becomes

```
type Color=enum(Red,Green,Blue)
type Shape_Circle=float64
type Shape_Empty=null
type Shape_Rect={w:float64,h:float64}
type Shape=Shape_Circle|Shape_Empty|Shape_Rect
type Point={x:int64,y:int64}
type Thing={name:string,color:Color,shapes:[Shape],at:Point|null}
```

so the values read back into the same Rust types, and SuperDB can tell them
apart (e.g., `unnest shapes | where is(this, <Shape_Rect>)`).  See the
`schema` module for the full mapping.  Types that can't be traced (e.g.,
untagged enums, which deserialize with `deserialize_any`) fall back to types
inferred from each value.

```rust
let bytes = superdb::to_vec(&things)?;
let things: Vec<Thing> = superdb::from_slice(&bytes)?;

// Or stream them.
let mut w = superdb::bsup::Writer::new(file);
w.write(&thing)?;
w.finish()?;
for thing in superdb::bsup::Reader::new(file).deserialize::<Thing>() {
    let thing = thing?;
}
```

## Status

* Reading handles everything `super` writes in BSUP 0.31, in row or column
  frames, and is tested against files written by `super`.
* Writing produces row frames, whose fused types match those `super` writes.
  Writing column frames isn't implemented yet.
* Zone maps in super footers are skipped.

`cargo run --example bsupcat -- -s FILE.bsup` prints a BSUP file as SUP.
