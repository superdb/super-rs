//! Tests that this crate reads the BSUP that SuperDB writes (in row frames
//! and column frames) and writes BSUP whose values and fused types match
//! SuperDB's.  The files in tests/data were written by SuperDB (see
//! tests/data/README.md).

use std::fs;
use std::path::PathBuf;

use superdb::bsup::{Reader, Writer};
use superdb::sup::Formatter;
use superdb::Value;

fn data(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/data")
        .join(name)
}

fn read(name: &str) -> (Vec<Value>, Reader<std::io::Cursor<Vec<u8>>>) {
    let mut r = Reader::new(std::io::Cursor::new(fs::read(data(name)).unwrap()));
    let mut vals = Vec::new();
    while let Some(v) = r.next_value().unwrap() {
        vals.push(v);
    }
    (vals, r)
}

fn sup(vals: &[Value]) -> String {
    let mut f = Formatter::new();
    vals.iter().map(|v| f.format(v) + "\n").collect()
}

fn check(stem: &str) {
    let expected = fs::read_to_string(data(&format!("{stem}.expected.sup"))).unwrap();
    let (rows, go_rows) = read(&format!("{stem}.rows.bsup"));
    let (cols, _) = read(&format!("{stem}.cols.bsup"));
    // Both encodings decode to the values SuperDB shows.
    assert_eq!(sup(&rows), expected);
    assert_eq!(sup(&cols), expected);
    // Writing the values reproduces them and SuperDB's fused type.
    let mut w = Writer::new(Vec::new());
    for v in &cols {
        w.write_value(v).unwrap();
    }
    let bytes = w.finish().unwrap();
    let mut r = Reader::new(&bytes[..]);
    let mut again = Vec::new();
    while let Some(v) = r.next_value().unwrap() {
        again.push(v);
    }
    assert_eq!(again, cols);
    assert_eq!(r.fused_types(), go_rows.fused_types());
}

#[test]
fn types() {
    check("types");
}

#[test]
fn mixed() {
    check("mixed");
}
