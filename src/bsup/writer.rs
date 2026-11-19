//! The BSUP writer, which writes row frames.

use std::collections::HashMap;
use std::io::Write;

use super::{
    version_bytes, CONTROL_MAGIC, ROW_HEADER_SIZE, ROW_MAGIC, SUPER_FOOTER_SIZE,
    SUPER_FOOTER_TRAILER, SUPER_MAGIC,
};
use crate::error::{Error, Result};
use crate::fuser::Fuser;
use crate::typedefs::TypeDefsEncoder;
use crate::types::Type;
use crate::typevalue::encode_type_value;
use crate::value::Value;
use crate::varint::put_uvarint;

/// Options for a [`Writer`].
#[derive(Clone, Debug)]
pub struct WriterOptions {
    /// The maximum number of values in a frame.
    pub frame_values: usize,
    /// The size in bytes of a frame's values section above which the frame
    /// is ended.
    pub frame_bytes: usize,
    /// The number of frames after which the super frame is ended.  Each
    /// super frame ends with a footer that summarizes it.  `None` means the
    /// super frame ends only when [`Writer::end_super_frame`] or
    /// [`Writer::finish`] is called.
    pub super_frame_frames: Option<usize>,
}

impl Default for WriterOptions {
    fn default() -> Self {
        WriterOptions {
            frame_values: 20_000,
            frame_bytes: 8 * 1024 * 1024,
            super_frame_frames: None,
        }
    }
}

/// Writes super-structured values to BSUP row frames.
///
/// Values are buffered into frames, so [`Writer::finish`] must be called to
/// write the last frame and the super footer.  (Dropping the writer calls
/// `finish` but ignores any error.)
///
/// A type name may be bound to only one type in a BSUP file (as in a
/// SuperDB type context), so writing a value whose type binds a name to a
/// type other than the one it was bound to earlier fails with
/// [`Error::Redefined`].
pub struct Writer<W: Write> {
    w: Option<W>,
    opts: WriterOptions,
    typedefs: TypeDefsEncoder,
    values: Vec<u8>,
    count: usize,
    frames: usize,
    frame_fuser: Fuser,
    super_fuser: Fuser,
    size: u64,
    names: HashMap<String, Type>,
    scratch: Vec<u8>,
}

impl<W: Write> Writer<W> {
    /// Returns a writer to `w` with the default options.
    pub fn new(w: W) -> Writer<W> {
        Writer::with_options(w, WriterOptions::default())
    }

    /// Returns a writer to `w` with `opts`.
    pub fn with_options(w: W, opts: WriterOptions) -> Writer<W> {
        Writer {
            w: Some(w),
            opts,
            typedefs: TypeDefsEncoder::new(),
            values: Vec::new(),
            count: 0,
            frames: 0,
            frame_fuser: Fuser::new(),
            super_fuser: Fuser::new(),
            size: 0,
            names: HashMap::new(),
            scratch: Vec::new(),
        }
    }

    /// Writes `value`.
    pub fn write_value(&mut self, value: &Value) -> Result<()> {
        let ty = value.typ();
        self.check_names(&ty)?;
        self.scratch.clear();
        value.encode_body(&mut self.scratch)?;
        let id = self.typedefs.lookup(&ty)?;
        put_uvarint(&mut self.values, u64::from(id));
        put_uvarint(&mut self.values, self.scratch.len() as u64);
        self.values.extend_from_slice(&self.scratch);
        self.frame_fuser.fuse(&ty);
        self.super_fuser.fuse(&ty);
        self.count += 1;
        if self.count >= self.opts.frame_values || self.values.len() >= self.opts.frame_bytes {
            self.end_frame()?;
        }
        Ok(())
    }

    /// Writes a Rust value with the static type of `T` (see
    /// [`crate::to_value`]).
    pub fn write<T: serde::Serialize + serde::de::DeserializeOwned>(
        &mut self,
        value: &T,
    ) -> Result<()> {
        self.write_value(&crate::to_value(value)?)
    }

    /// Writes a Rust value with a type inferred from the value (see
    /// [`crate::to_value_untyped`]).
    pub fn write_untyped<T: serde::Serialize + ?Sized>(&mut self, value: &T) -> Result<()> {
        self.write_value(&crate::to_value_untyped(value)?)
    }

    /// Writes a control message, which a reader delivers in order with the
    /// values (see [`super::Item`]).  It ends the current frame and writes
    /// the message in a control frame of its own.
    pub fn write_control(&mut self, value: &Value) -> Result<()> {
        self.end_frame()?;
        let ty = value.typ();
        self.check_names(&ty)?;
        let mut typedefs = TypeDefsEncoder::new();
        let mut body = Vec::new();
        value.encode_body(&mut body)?;
        let mut values = Vec::new();
        put_uvarint(&mut values, u64::from(typedefs.lookup(&ty)?));
        put_uvarint(&mut values, body.len() as u64);
        values.extend_from_slice(&body);
        let fused = typedefs.lookup(&ty)?;
        self.write_frame(CONTROL_MAGIC, fused, &typedefs.encode(), &values)
    }

    /// Ends the current frame, writing it if it has any values.
    pub fn end_frame(&mut self) -> Result<()> {
        if self.values.is_empty() {
            return Ok(());
        }
        let fused = self
            .frame_fuser
            .fused_type()
            .expect("a frame with values has a fused type");
        let fused = self.typedefs.lookup(&fused)?;
        let typedefs = self.typedefs.encode();
        let values = std::mem::take(&mut self.values);
        self.write_frame(ROW_MAGIC, fused, &typedefs, &values)?;
        self.values = values;
        self.values.clear();
        self.typedefs.reset();
        self.frame_fuser = Fuser::new();
        self.count = 0;
        self.frames += 1;
        if let Some(n) = self.opts.super_frame_frames {
            if self.frames >= n {
                self.end_super_frame()?;
            }
        }
        Ok(())
    }

    fn write_frame(
        &mut self,
        magic: &[u8; 4],
        fused: u32,
        typedefs: &[u8],
        values: &[u8],
    ) -> Result<()> {
        let frame_size = (ROW_HEADER_SIZE + typedefs.len() + values.len()) as u64;
        let mut header = [0u8; ROW_HEADER_SIZE];
        header[..4].copy_from_slice(magic);
        header[4..6].copy_from_slice(&version_bytes());
        header[6..14].copy_from_slice(&frame_size.to_le_bytes());
        header[14..18].copy_from_slice(&fused.to_le_bytes());
        header[18..26].copy_from_slice(&(typedefs.len() as u64).to_le_bytes());
        let w = self
            .w
            .as_mut()
            .ok_or_else(|| Error::msg("BSUP writer is finished"))?;
        w.write_all(&header)?;
        w.write_all(typedefs)?;
        w.write_all(values)?;
        self.size += frame_size;
        Ok(())
    }

    /// Ends the current frame and the current super frame, writing its
    /// super footer.  Values written afterward start a new super frame.
    pub fn end_super_frame(&mut self) -> Result<()> {
        self.end_frame()?;
        if self.size == 0 {
            return Ok(());
        }
        let fused = match self.super_fuser.fused_type() {
            Some(t) => encode_type_value(&t)?,
            None => Vec::new(),
        };
        let footer_size = (SUPER_FOOTER_SIZE + fused.len() + SUPER_FOOTER_TRAILER) as u64;
        let mut footer = Vec::with_capacity(footer_size as usize);
        footer.extend_from_slice(SUPER_MAGIC);
        footer.extend_from_slice(&version_bytes());
        footer.extend_from_slice(&footer_size.to_le_bytes());
        footer.extend_from_slice(&(fused.len() as u64).to_le_bytes());
        // A super frame of row frames has no zone maps.
        footer.extend_from_slice(&0u64.to_le_bytes());
        footer.extend_from_slice(&self.size.to_le_bytes());
        footer.extend_from_slice(&fused);
        footer.extend_from_slice(&footer_size.to_le_bytes());
        let w = self
            .w
            .as_mut()
            .ok_or_else(|| Error::msg("BSUP writer is finished"))?;
        w.write_all(&footer)?;
        self.size = 0;
        self.frames = 0;
        self.super_fuser = Fuser::new();
        Ok(())
    }

    /// Ends the super frame, flushes the underlying writer, and returns it.
    pub fn finish(mut self) -> Result<W> {
        self.end_super_frame()?;
        let mut w = self.w.take().expect("writer is present until finished");
        w.flush()?;
        Ok(w)
    }

    /// Returns a reference to the underlying writer.
    pub fn get_ref(&self) -> &W {
        self.w.as_ref().expect("writer is present until finished")
    }

    fn check_names(&mut self, ty: &Type) -> Result<()> {
        let mut seen = Vec::new();
        check_names(ty, &mut self.names, &mut seen)
    }
}

fn check_names(ty: &Type, names: &mut HashMap<String, Type>, seen: &mut Vec<String>) -> Result<()> {
    match ty {
        Type::Primitive(_) | Type::Enum(_) | Type::Decimal(..) => Ok(()),
        Type::Record(fields) => fields
            .iter()
            .try_for_each(|f| check_names(&f.ty, names, seen)),
        Type::Array(t) | Type::Set(t) | Type::Error(t) | Type::Fusion(t) | Type::Option(t) => {
            check_names(t, names, seen)
        }
        Type::Map(kv) => {
            check_names(&kv.0, names, seen)?;
            check_names(&kv.1, names, seen)
        }
        Type::Union(types) => types.iter().try_for_each(|t| check_names(t, names, seen)),
        Type::Named(n) => {
            if seen.iter().any(|s| s == n.name()) {
                return Ok(());
            }
            seen.push(n.name().to_string());
            match names.get(n.name()) {
                Some(bound) if bound != ty => return Err(Error::Redefined(n.name().to_string())),
                Some(_) => {}
                None => {
                    names.insert(n.name().to_string(), ty.clone());
                }
            }
            check_names(n.inner(), names, seen)
        }
    }
}

impl<W: Write> Drop for Writer<W> {
    fn drop(&mut self) {
        if self.w.is_some() {
            let _ = self.end_super_frame();
            if let Some(w) = self.w.as_mut() {
                let _ = w.flush();
            }
        }
    }
}
