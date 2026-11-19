//! The BSUP reader, which reads row frames and column frames.

use std::collections::VecDeque;
use std::io::{self, Read};

use super::column::decode_column_frame;
use super::{
    check_version, le32, le64, COLUMN_HEADER_SIZE, COLUMN_MAGIC, CONTROL_MAGIC, MAX_FRAME_SIZE,
    MAX_META_SIZE, ROW_HEADER_SIZE, ROW_MAGIC, SUPER_FOOTER_SIZE, SUPER_MAGIC,
};
use crate::error::{Error, Result};
use crate::typedefs::TypeDefs;
use crate::types::Type;
use crate::typevalue::decode_type_value;
use crate::value::Value;
use crate::varint::Cursor;

/// An item of a BSUP file: a value or a control message.
#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    /// A value.
    Value(Value),
    /// A control message (see [`super::Writer::write_control`]).
    Control(Value),
}

/// Reads super-structured values from a BSUP file.
///
/// The reader reads the file sequentially, one frame at a time, so it
/// doesn't need to seek and a frame's values are decoded when the frame is
/// read.  Iterating over a reader yields its values (skipping control
/// messages).
pub struct Reader<R: Read> {
    r: R,
    queue: VecDeque<Item>,
    fused_types: Vec<Option<Type>>,
    done: bool,
}

impl<R: Read> Reader<R> {
    /// Returns a reader of `r`.
    pub fn new(r: R) -> Reader<R> {
        Reader {
            r,
            queue: VecDeque::new(),
            fused_types: Vec::new(),
            done: false,
        }
    }

    /// Returns the next value, skipping control messages, or `None` at the
    /// end of the input.
    pub fn next_value(&mut self) -> Result<Option<Value>> {
        loop {
            match self.next_item()? {
                Some(Item::Value(v)) => return Ok(Some(v)),
                Some(Item::Control(_)) => continue,
                None => return Ok(None),
            }
        }
    }

    /// Returns the next value or control message, or `None` at the end of
    /// the input.
    pub fn next_item(&mut self) -> Result<Option<Item>> {
        loop {
            if let Some(item) = self.queue.pop_front() {
                return Ok(Some(item));
            }
            if self.done {
                return Ok(None);
            }
            if let Err(err) = self.read_frame() {
                self.done = true;
                return Err(err);
            }
        }
    }

    /// Returns the next value converted to a Rust value (see
    /// [`crate::from_value`]), or `None` at the end of the input.
    pub fn read<T: serde::de::DeserializeOwned>(&mut self) -> Result<Option<T>> {
        match self.next_value()? {
            Some(v) => crate::from_value(&v).map(Some),
            None => Ok(None),
        }
    }

    /// Returns an iterator over the values converted to Rust values.
    pub fn deserialize<T: serde::de::DeserializeOwned>(self) -> impl Iterator<Item = Result<T>> {
        let mut r = self;
        std::iter::from_fn(move || r.read().transpose())
    }

    /// Returns the fused types of the super frames read so far (from their
    /// super footers), in order.  A super frame of only control messages has
    /// no fused type.
    pub fn fused_types(&self) -> &[Option<Type>] {
        &self.fused_types
    }

    /// Returns the underlying reader.
    pub fn into_inner(self) -> R {
        self.r
    }

    fn read_frame(&mut self) -> Result<()> {
        let mut magic = [0u8; 4];
        if !read_magic(&mut self.r, &mut magic)? {
            self.done = true;
            return Ok(());
        }
        match &magic {
            m if m == ROW_MAGIC || m == CONTROL_MAGIC => {
                let mut h = [0u8; ROW_HEADER_SIZE];
                self.r.read_exact(&mut h[4..]).map_err(short)?;
                check_version([h[4], h[5]])?;
                let frame_size = le64(&h[6..]);
                let fused = le32(&h[14..]);
                let typedefs_size = le64(&h[18..]);
                check_size("row frame", frame_size, MAX_FRAME_SIZE)?;
                check_size("typedefs section", typedefs_size, MAX_META_SIZE)?;
                let body_size = frame_size
                    .checked_sub(ROW_HEADER_SIZE as u64)
                    .filter(|&n| n >= typedefs_size)
                    .ok_or_else(|| Error::malformed("bad row frame size"))?;
                let body = read_n(&mut self.r, body_size)?;
                let (typedefs, values) = body.split_at(typedefs_size as usize);
                let defs = TypeDefs::decode(typedefs)?;
                defs.lookup(fused)?;
                let control = m == CONTROL_MAGIC;
                let mut c = Cursor::new(values);
                while !c.is_empty() {
                    let id = c.uvarint_u32()?;
                    let ty = defs.lookup(id)?;
                    let v = Value::decode(&ty, c.body()?)?;
                    self.queue.push_back(if control {
                        Item::Control(v)
                    } else {
                        Item::Value(v)
                    });
                }
            }
            m if m == COLUMN_MAGIC => {
                let mut h = [0u8; COLUMN_HEADER_SIZE];
                self.r.read_exact(&mut h[4..]).map_err(short)?;
                check_version([h[4], h[5]])?;
                let frame_size = le64(&h[6..]);
                let fused = le32(&h[14..]);
                let vector_typedefs_size = le64(&h[18..]);
                let locators_size = le64(&h[26..]);
                let type_values_size = le64(&h[34..]);
                check_size("column frame", frame_size, MAX_FRAME_SIZE)?;
                check_size(
                    "vector typedefs section",
                    vector_typedefs_size,
                    MAX_META_SIZE,
                )?;
                check_size("locators section", locators_size, MAX_META_SIZE)?;
                check_size("type values section", type_values_size, MAX_META_SIZE)?;
                let meta = vector_typedefs_size + locators_size + type_values_size;
                let body_size = frame_size
                    .checked_sub(COLUMN_HEADER_SIZE as u64)
                    .filter(|&n| n >= meta)
                    .ok_or_else(|| Error::malformed("bad column frame size"))?;
                let body = read_n(&mut self.r, body_size)?;
                let (vector_typedefs, rest) = body.split_at(vector_typedefs_size as usize);
                let (locators, rest) = rest.split_at(locators_size as usize);
                let (type_values, data) = rest.split_at(type_values_size as usize);
                let values =
                    decode_column_frame(fused, vector_typedefs, locators, type_values, data)?;
                self.queue.extend(values.into_iter().map(Item::Value));
            }
            m if m == SUPER_MAGIC => {
                let mut h = [0u8; SUPER_FOOTER_SIZE];
                self.r.read_exact(&mut h[4..]).map_err(short)?;
                check_version([h[4], h[5]])?;
                let footer_size = le64(&h[6..]);
                let fused_size = le64(&h[14..]);
                let stats_size = le64(&h[22..]);
                check_size("super footer", footer_size, MAX_META_SIZE)?;
                let rest = footer_size
                    .checked_sub(SUPER_FOOTER_SIZE as u64)
                    .filter(|&n| n >= fused_size.saturating_add(stats_size).saturating_add(8))
                    .ok_or_else(|| Error::malformed("bad super footer size"))?;
                let rest = read_n(&mut self.r, rest)?;
                let fused = &rest[..fused_size as usize];
                self.fused_types.push(if fused.is_empty() {
                    None
                } else {
                    Some(decode_type_value(fused)?)
                });
                let trailer = le64(&rest[rest.len() - 8..]);
                if trailer != footer_size {
                    return Err(Error::malformed(
                        "super footer trailer doesn't match its size",
                    ));
                }
            }
            _ => {
                return Err(Error::malformed(format!(
                    "unknown frame magic {:?}",
                    String::from_utf8_lossy(&magic)
                )))
            }
        }
        Ok(())
    }
}

impl<R: Read> Iterator for Reader<R> {
    type Item = Result<Value>;

    fn next(&mut self) -> Option<Result<Value>> {
        self.next_value().transpose()
    }
}

/// Reads the 4-byte magic of the next frame, returning false at the end of
/// the input.
fn read_magic<R: Read>(r: &mut R, magic: &mut [u8; 4]) -> Result<bool> {
    let mut n = 0;
    while n < 4 {
        match r.read(&mut magic[n..]) {
            Ok(0) if n == 0 => return Ok(false),
            Ok(0) => return Err(Error::malformed("truncated frame")),
            Ok(k) => n += k,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(true)
}

fn read_n<R: Read>(r: &mut R, n: u64) -> Result<Vec<u8>> {
    let mut buf = Vec::with_capacity(n.min(64 * 1024 * 1024) as usize);
    r.take(n).read_to_end(&mut buf)?;
    if buf.len() as u64 != n {
        return Err(Error::malformed("truncated frame"));
    }
    Ok(buf)
}

fn short(err: io::Error) -> Error {
    if err.kind() == io::ErrorKind::UnexpectedEof {
        Error::malformed("truncated frame header")
    } else {
        err.into()
    }
}

fn check_size(what: &str, size: u64, max: u64) -> Result<()> {
    if size > max {
        return Err(Error::malformed(format!(
            "{what} size {size} exceeds maximum {max}"
        )));
    }
    Ok(())
}
