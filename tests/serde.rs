use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use superdb::bsup::{Reader, Writer};
use superdb::{from_value, to_value, to_value_untyped, type_of, Timestamp, Value};

#[derive(Debug, PartialEq, Clone, Serialize, Deserialize)]
struct Point {
    x: i64,
    y: i64,
}

#[derive(Debug, PartialEq, Clone, Serialize, Deserialize)]
enum Color {
    Red,
    Green,
    Blue,
}

#[derive(Debug, PartialEq, Clone, Serialize, Deserialize)]
enum Shape {
    Circle(f64),
    Rect { w: f64, h: f64 },
    Empty,
}

#[derive(Debug, PartialEq, Clone, Serialize, Deserialize)]
struct Thing {
    name: String,
    color: Color,
    shapes: Vec<Shape>,
    at: Option<Point>,
    tags: BTreeMap<String, u32>,
    when: Timestamp,
    small: u8,
}

#[derive(Debug, PartialEq, Clone, Serialize, Deserialize)]
struct Tree {
    label: String,
    kids: Vec<Tree>,
    next: Option<Box<Tree>>,
}

#[derive(Debug, PartialEq, Clone, Serialize, Deserialize)]
enum Expr {
    Neg(Box<Expr>),
    Lit(i64),
}

fn thing() -> Thing {
    Thing {
        name: "widget".into(),
        color: Color::Green,
        shapes: vec![
            Shape::Circle(1.5),
            Shape::Rect { w: 2.0, h: 3.0 },
            Shape::Empty,
        ],
        at: Some(Point { x: 1, y: 2 }),
        tags: [("a".to_string(), 1), ("b".to_string(), 2)]
            .into_iter()
            .collect(),
        when: Timestamp(1_700_000_000_000_000_000),
        small: 7,
    }
}

#[test]
fn static_types() {
    assert_eq!(
        format!("{:?}", type_of::<Point>().unwrap()),
        "Point={x:int64,y:int64}"
    );
    assert_eq!(
        format!("{:?}", type_of::<Color>().unwrap()),
        "Color=enum(Red,Green,Blue)"
    );
    assert_eq!(
        format!("{:?}", type_of::<Shape>().unwrap()),
        "Shape=(Shape_Circle=float64)|(Shape_Empty=null)|(Shape_Rect={w:float64,h:float64})"
    );
    assert_eq!(
        format!("{:?}", type_of::<Thing>().unwrap()),
        "Thing={name:string,color:Color=enum(Red,Green,Blue),shapes:[Shape=(Shape_Circle=float64)|(Shape_Empty=null)|(Shape_Rect={w:float64,h:float64})],at?:Point={x:int64,y:int64},tags:map{string:uint32},when:timestamp,small:uint8}"
    );
    assert_eq!(
        format!("{:?}", type_of::<Tree>().unwrap()),
        "Tree={label:string,kids:[Tree],next?:Tree}"
    );
    assert_eq!(
        format!("{:?}", type_of::<Expr>().unwrap()),
        "Expr=(Expr_Lit=int64)|(Expr_Neg=Expr)"
    );
    assert_eq!(
        format!("{:?}", type_of::<Option<Vec<(i32, String)>>>().unwrap()),
        "option([{column1:int32,column2:string}])"
    );
}

#[test]
fn typed_roundtrip() {
    let t = thing();
    let v = to_value(&t).unwrap();
    assert_eq!(v.typ(), type_of::<Thing>().unwrap());
    assert_eq!(
        v.to_string(),
        r#"{name:"widget",color:"Green"::Color,shapes:[1.5::Shape_Circle::Shape,{w:2.,h:3.}::Shape_Rect::Shape,null::Shape_Empty::Shape],at?:{x:1,y:2}::Point,tags:map{"a":1::uint32,"b":2::uint32},when:2023-11-14T22:13:20Z,small:7::uint8}::Thing"#
    );
    assert_eq!(from_value::<Thing>(&v).unwrap(), t);
    // Values of the same Rust type have the same type.
    let mut t2 = thing();
    t2.at = None;
    t2.shapes.clear();
    assert_eq!(to_value(&t2).unwrap().typ(), v.typ());
    assert_eq!(from_value::<Thing>(&to_value(&t2).unwrap()).unwrap(), t2);
}

#[test]
fn recursive_roundtrip() {
    let tree = Tree {
        label: "root".into(),
        kids: vec![Tree {
            label: "kid".into(),
            kids: vec![],
            next: None,
        }],
        next: Some(Box::new(Tree {
            label: "next".into(),
            kids: vec![],
            next: None,
        })),
    };
    let v = to_value(&tree).unwrap();
    assert_eq!(from_value::<Tree>(&v).unwrap(), tree);
    let e = Expr::Neg(Box::new(Expr::Neg(Box::new(Expr::Lit(3)))));
    let v = to_value(&e).unwrap();
    assert_eq!(from_value::<Expr>(&v).unwrap(), e);
    // Recursive named types survive BSUP.
    let bytes = superdb::to_vec(std::slice::from_ref(&tree)).unwrap();
    assert_eq!(superdb::from_slice::<Tree>(&bytes).unwrap(), vec![tree]);
}

#[test]
fn untyped_roundtrip() {
    let t = thing();
    let v = to_value_untyped(&t).unwrap();
    assert_eq!(
        v.to_string(),
        r#"{name:"widget",color:"Green",shapes:[{Circle:1.5},{Rect:{w:2.,h:3.}},"Empty"],at?:{x:1,y:2},tags:map{"a":1::uint32,"b":2::uint32},when:2023-11-14T22:13:20Z,small:7::uint8}"#
    );
    assert_eq!(from_value::<Thing>(&v).unwrap(), t);
}

#[test]
fn bsup_roundtrip() {
    let things = vec![
        thing(),
        Thing {
            at: None,
            shapes: vec![],
            ..thing()
        },
    ];
    let mut w = Writer::new(Vec::new());
    for t in &things {
        w.write(t).unwrap();
    }
    w.write(&Point { x: 9, y: 9 }).unwrap();
    let bytes = w.finish().unwrap();
    let mut r = Reader::new(&bytes[..]);
    assert_eq!(r.read::<Thing>().unwrap().unwrap(), things[0]);
    assert_eq!(r.read::<Thing>().unwrap().unwrap(), things[1]);
    assert_eq!(r.read::<Point>().unwrap().unwrap(), Point { x: 9, y: 9 });
    assert!(r.read::<Point>().unwrap().is_none());
}

#[test]
fn std_types() {
    let ip: std::net::IpAddr = "10.0.0.1".parse().unwrap();
    let v = to_value(&ip).unwrap();
    assert_eq!(v, Value::Ip(ip));
    assert_eq!(from_value::<std::net::IpAddr>(&v).unwrap(), ip);
    let pair = (1i32, "x".to_string());
    let v = to_value(&pair).unwrap();
    assert_eq!(v.to_string(), r#"{column1:1::int32,column2:"x"}"#);
    assert_eq!(from_value::<(i32, String)>(&v).unwrap(), pair);
    let opts = vec![Some(1i64), None];
    let v = to_value(&opts).unwrap();
    assert_eq!(v.to_string(), "[some(1),none::int64]");
    assert_eq!(format!("{:?}", v.typ()), "[option(int64)]");
    assert_eq!(from_value::<Vec<Option<i64>>>(&v).unwrap(), opts);
}

#[test]
fn shapes_from_other_sources() {
    // A variant as a value of a named type, an enum value, or a string.
    let v = Value::String("Blue".into());
    assert_eq!(from_value::<Color>(&v).unwrap(), Color::Blue);
    let v = Value::Enum(vec!["Red".to_string(), "Blue".to_string()].into(), 1);
    assert_eq!(from_value::<Color>(&v).unwrap(), Color::Blue);
    // Missing and absent fields.
    let v = Value::record([
        ("x", Value::Int64(1)),
        ("y", Value::Int64(2)),
        ("z", Value::Missing),
    ]);
    assert_eq!(from_value::<Point>(&v).unwrap(), Point { x: 1, y: 2 });
    assert!(from_value::<Point>(&Value::record([("x", Value::Int64(1))])).is_err());
}

#[derive(Debug, PartialEq, Clone, Serialize, Deserialize)]
struct Opts {
    a: Option<i64>,
    b: Option<Point>,
    c: Option<Option<String>>,
    d: Option<()>,
    e: Vec<Option<u8>>,
}

#[test]
fn options() {
    // An Option is an option type, so an Option field is an optional field.
    assert_eq!(
        format!("{:?}", type_of::<Opts>().unwrap()),
        "Opts={a?:int64,b?:Point={x:int64,y:int64},c?:string,d?:null,e:[option(uint8)]}"
    );
    let some = Opts {
        a: Some(1),
        b: Some(Point { x: 1, y: 2 }),
        c: Some(Some("s".into())),
        d: Some(()),
        e: vec![None, Some(2)],
    };
    let none = Opts {
        a: None,
        b: None,
        c: None,
        d: None,
        e: vec![None],
    };
    let v = to_value(&some).unwrap();
    assert_eq!(
        v.to_string(),
        r#"{a?:1,b?:{x:1,y:2}::Point,c?:"s",d?:null,e:[none::uint8,some(2::uint8)]}::Opts"#
    );
    assert_eq!(v.typ(), type_of::<Opts>().unwrap());
    assert_eq!(from_value::<Opts>(&v).unwrap(), some);
    let v = to_value(&none).unwrap();
    assert_eq!(
        v.to_string(),
        "{a?:none::int64,b?:none::Point,c?:none::string,d?:none::null,e:[none::uint8]}::Opts"
    );
    assert_eq!(v.typ(), type_of::<Opts>().unwrap());
    assert_eq!(from_value::<Opts>(&v).unwrap(), none);
    // There's no option of an option, so Some(None) is none.
    let flat = Opts {
        c: Some(None),
        ..some.clone()
    };
    let back = from_value::<Opts>(&to_value(&flat).unwrap()).unwrap();
    assert_eq!(
        back,
        Opts {
            c: None,
            ..some.clone()
        }
    );
    // Options survive BSUP.
    let all = vec![some.clone(), none.clone()];
    let bytes = superdb::to_vec(&all).unwrap();
    assert_eq!(superdb::from_slice::<Opts>(&bytes).unwrap(), all);
    // Without static types, a None has the option type of its neighbors.
    let v = to_value_untyped(&some).unwrap();
    assert_eq!(
        v.to_string(),
        r#"{a?:1,b?:{x:1,y:2},c?:"s",d?:null,e:[none::uint8,some(2::uint8)]}"#
    );
    assert_eq!(from_value::<Opts>(&v).unwrap(), some);
    let v = to_value_untyped(&none).unwrap();
    assert_eq!(v.to_string(), "{a:none,b:none,c:none,d:none,e:[none]}");
    assert_eq!(from_value::<Opts>(&v).unwrap(), none);
}

#[test]
fn options_from_other_sources() {
    use superdb::Type;
    // Nullable values (e.g., from a SQL table or written when an Option
    // was T|null), absent fields, and missing fields are None.
    let nullable = Type::union([Type::INT64, Type::NULL]).unwrap();
    let null = |v: Value| Value::Union(nullable.clone(), Box::new(v));
    let rec = |a: Value| {
        Value::record([
            ("a", a),
            ("b", Value::Missing),
            ("d", Value::Null),
            ("e", Value::Array(Type::NONE, vec![])),
        ])
    };
    let want = |a| Opts {
        a,
        b: None,
        c: None,
        d: None,
        e: vec![],
    };
    assert_eq!(
        from_value::<Opts>(&rec(null(Value::Int64(5)))).unwrap(),
        want(Some(5))
    );
    assert_eq!(
        from_value::<Opts>(&rec(null(Value::Null))).unwrap(),
        want(None)
    );
    assert_eq!(
        from_value::<Opts>(&rec(Value::Int64(5))).unwrap(),
        want(Some(5))
    );
    assert_eq!(from_value::<Opts>(&rec(Value::None)).unwrap(), want(None));
    let some = Value::Option(Type::INT64, Some(Box::new(Value::Int64(5))));
    assert_eq!(
        from_value::<Opts>(&rec(some.clone())).unwrap(),
        want(Some(5))
    );
    assert_eq!(
        from_value::<Opts>(&rec(Value::Option(Type::INT64, None))).unwrap(),
        want(None)
    );
    // An option value is read into a type that isn't an Option if it's
    // present.
    assert_eq!(from_value::<i64>(&some).unwrap(), 5);
    assert!(from_value::<i64>(&Value::Option(Type::INT64, None)).is_err());
}

/// Writes superdb-serde.bsup in the temporary directory for checking with
/// SuperDB (e.g., `super -s FILE`).  Run with `cargo test -- --ignored`.
#[test]
#[ignore]
fn write_interop_file() {
    let things = vec![
        thing(),
        Thing {
            at: None,
            shapes: vec![Shape::Empty],
            color: Color::Red,
            ..thing()
        },
    ];
    let tree = Tree {
        label: "root".into(),
        kids: vec![],
        next: Some(Box::new(Tree {
            label: "n".into(),
            kids: vec![],
            next: None,
        })),
    };
    let mut w = Writer::new(Vec::new());
    for t in &things {
        w.write(t).unwrap();
    }
    w.write(&tree).unwrap();
    w.write(&Expr::Neg(Box::new(Expr::Lit(3)))).unwrap();
    let bytes = w.finish().unwrap();
    std::fs::write(std::env::temp_dir().join("superdb-serde.bsup"), bytes).unwrap();
}

#[derive(Debug, PartialEq, Clone, Serialize, Deserialize)]
#[serde(rename = "Renamed")]
struct Attrs {
    #[serde(skip_serializing_if = "Option::is_none")]
    maybe: Option<u16>,
    #[serde(rename = "kind")]
    k: Kind,
    pair: Pair,
    meters: Meters,
    unit: Unit,
}

#[derive(Debug, PartialEq, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Kind {
    Big,
    Small,
}

#[derive(Debug, PartialEq, Clone, Serialize, Deserialize)]
struct Pair(i8, String);

#[derive(Debug, PartialEq, Clone, Serialize, Deserialize)]
struct Meters(f64);

#[derive(Debug, PartialEq, Clone, Serialize, Deserialize)]
struct Unit;

#[derive(Debug, PartialEq, Clone, Serialize, Deserialize)]
#[serde(untagged)]
enum Loose {
    Num(i64),
    Text(String),
}

#[derive(Debug, PartialEq, Clone, Serialize, Deserialize)]
struct Wrapper<T> {
    inner: T,
}

#[derive(Debug, PartialEq, Clone, Serialize, Deserialize)]
struct Two {
    a: Wrapper<i64>,
    b: Wrapper<String>,
}

#[test]
fn serde_attributes() {
    assert_eq!(
        format!("{:?}", type_of::<Attrs>().unwrap()),
        "Renamed={maybe?:uint16,kind:Kind=enum(big,small),pair:Pair={column1:int8,column2:string},meters:Meters=float64,unit:Unit=null}"
    );
    let a = Attrs {
        maybe: None,
        k: Kind::Small,
        pair: Pair(-1, "p".into()),
        meters: Meters(2.5),
        unit: Unit,
    };
    let v = to_value(&a).unwrap();
    assert_eq!(v.typ(), type_of::<Attrs>().unwrap());
    // The skipped field is none.
    assert!(v.to_string().starts_with("{maybe?:none::uint16,"));
    assert_eq!(from_value::<Attrs>(&v).unwrap(), a);
}

#[test]
fn untraceable_types_are_inferred() {
    // An untagged enum deserializes with deserialize_any, so its type is
    // inferred from each value.
    assert!(type_of::<Loose>().is_err());
    let v = to_value(&vec![Loose::Num(1), Loose::Text("x".into())]).unwrap();
    assert_eq!(v.to_string(), r#"[1,"x"]"#);
    assert_eq!(
        from_value::<Vec<Loose>>(&v).unwrap(),
        vec![Loose::Num(1), Loose::Text("x".into())]
    );
}

#[test]
fn generic_name_conflict() {
    let two = Two {
        a: Wrapper { inner: 1 },
        b: Wrapper { inner: "x".into() },
    };
    assert!(matches!(to_value(&two), Err(superdb::Error::Redefined(name)) if name == "Wrapper"));
    // Without static types, the value can still be written.
    let v = to_value_untyped(&two).unwrap();
    assert_eq!(from_value::<Two>(&v).unwrap(), two);
}
