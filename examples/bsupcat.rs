//! Reads BSUP files (row or column frames) and writes their values to
//! standard output as BSUP row frames, as SUP text with `-s`, or prints the
//! fused types of their super frames with `-t`.
//!
//! ```text
//! cargo run --example bsupcat -- [-s] file.bsup ...
//! ```

use std::fs::File;
use std::io::{self, BufReader, BufWriter, Write};

use superdb::bsup::{Reader, Writer};

fn main() -> superdb::Result<()> {
    let mut sup = false;
    let mut types = false;
    let mut paths = Vec::new();
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "-s" => sup = true,
            "-t" => types = true,
            _ => paths.push(arg),
        }
    }
    if types {
        // Print the fused types of the files' super frames.
        for path in &paths {
            let mut r = Reader::new(BufReader::new(File::open(path)?));
            while r.next_item()?.is_some() {}
            for t in r.fused_types() {
                match t {
                    Some(t) => println!("{t:?}"),
                    None => println!("(none)"),
                }
            }
        }
        return Ok(());
    }
    let stdout = io::stdout().lock();
    if sup {
        let mut out = BufWriter::new(stdout);
        let mut f = superdb::sup::Formatter::new();
        for path in &paths {
            for v in Reader::new(BufReader::new(File::open(path)?)) {
                writeln!(out, "{}", f.format(&v?))?;
            }
        }
        out.flush()?;
    } else {
        let mut w = Writer::new(BufWriter::new(stdout));
        for path in &paths {
            for v in Reader::new(BufReader::new(File::open(path)?)) {
                w.write_value(&v?)?;
            }
        }
        w.finish()?;
    }
    Ok(())
}
