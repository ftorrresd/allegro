//! A cursor over a decompressed ROOT record.
//!
//! ROOT serialises everything big-endian. Composite objects are prefixed with
//! a byte count (high bit `0x40000000` set) followed by a 16-bit class
//! version, which lets a reader skip members it does not model.

use std::collections::HashMap;

use crate::error::{Error, Result};

pub const K_BYTE_COUNT_MASK: u32 = 0x4000_0000;
pub const K_CLASS_MASK: u32 = 0x8000_0000;
pub const K_NEW_CLASS_TAG: u32 = 0xFFFF_FFFF;
/// ROOT offsets object/class map keys by this amount.
pub const K_MAP_OFFSET: u32 = 2;

pub struct RBuffer<'a> {
    data: &'a [u8],
    pos: usize,
    /// Displacement of `data[0]` in ROOT's own coordinates.
    ///
    /// ROOT numbers class back-references from the start of the *key*, so a
    /// buffer holding only the object payload is shifted by the key length.
    origin: usize,
    /// Maps a ROOT displacement to the class name recorded there, for the
    /// back-references ROOT uses instead of repeating class names.
    classes: HashMap<u32, String>,
}

/// A byte-count/version header.
#[derive(Debug, Clone, Copy)]
pub struct ClassHeader {
    pub version: u16,
    /// Absolute buffer position just past this object's payload.
    pub end: usize,
}

impl<'a> RBuffer<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self::with_origin(data, 0)
    }

    /// Creates a buffer whose first byte sits at ROOT displacement `origin`.
    pub fn with_origin(data: &'a [u8], origin: usize) -> Self {
        Self {
            data,
            pos: 0,
            origin,
            classes: HashMap::new(),
        }
    }

    pub fn pos(&self) -> usize {
        self.pos
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    pub fn seek(&mut self, pos: usize) -> Result<()> {
        if pos > self.data.len() {
            return Err(Error::format(format!(
                "seek to {pos} past end of {}-byte record",
                self.data.len()
            )));
        }
        self.pos = pos;
        Ok(())
    }

    pub fn skip(&mut self, n: usize) -> Result<()> {
        self.seek(self.pos + n)
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.pos + n > self.data.len() {
            return Err(Error::format(format!(
                "read of {n} bytes at {} runs past the {}-byte record",
                self.pos,
                self.data.len()
            )));
        }
        let s = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    pub fn i8(&mut self) -> Result<i8> {
        Ok(self.u8()? as i8)
    }
    pub fn u16(&mut self) -> Result<u16> {
        let b = self.take(2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }
    pub fn i16(&mut self) -> Result<i16> {
        Ok(self.u16()? as i16)
    }
    pub fn u32(&mut self) -> Result<u32> {
        let b = self.take(4)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }
    pub fn i32(&mut self) -> Result<i32> {
        Ok(self.u32()? as i32)
    }
    pub fn u64(&mut self) -> Result<u64> {
        let b = self.take(8)?;
        Ok(u64::from_be_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }
    pub fn i64(&mut self) -> Result<i64> {
        Ok(self.u64()? as i64)
    }
    pub fn f32(&mut self) -> Result<f32> {
        Ok(f32::from_bits(self.u32()?))
    }
    pub fn f64(&mut self) -> Result<f64> {
        Ok(f64::from_bits(self.u64()?))
    }

    /// A ROOT `TString`: a one-byte length, or `255` followed by a 32-bit one.
    pub fn tstring(&mut self) -> Result<String> {
        let n = self.u8()?;
        let len = if n == 255 {
            self.u32()? as usize
        } else {
            n as usize
        };
        let b = self.take(len)?;
        Ok(String::from_utf8_lossy(b).into_owned())
    }

    /// A NUL-terminated string, as used for class names in object headers.
    pub fn cstring(&mut self) -> Result<String> {
        let start = self.pos;
        while self.pos < self.data.len() && self.data[self.pos] != 0 {
            self.pos += 1;
        }
        if self.pos >= self.data.len() {
            return Err(Error::format("unterminated class name"));
        }
        let s = String::from_utf8_lossy(&self.data[start..self.pos]).into_owned();
        self.pos += 1;
        Ok(s)
    }

    /// Reads a byte-count/version header and returns the version plus the
    /// absolute end of the object.
    pub fn class_header(&mut self) -> Result<ClassHeader> {
        let raw = self.u32()?;
        if raw & K_BYTE_COUNT_MASK == 0 {
            return Err(Error::format(format!(
                "expected a byte count at offset {}, found 0x{raw:08x}",
                self.pos - 4
            )));
        }
        let nbytes = (raw & !K_BYTE_COUNT_MASK) as usize;
        // The count covers everything after the 4-byte count itself.
        let end = self.pos + nbytes;
        let version = self.u16()?;
        Ok(ClassHeader { version, end })
    }

    /// Skips an embedded object by its byte count.
    pub fn skip_object(&mut self) -> Result<()> {
        let h = self.class_header()?;
        self.seek(h.end)
    }

    /// Reads the `TObject` base: version, `fUniqueID`, `fBits`.
    pub fn tobject(&mut self) -> Result<()> {
        let _version = self.u16()?;
        let _unique_id = self.u32()?;
        let bits = self.u32()?;
        // kIsReferenced adds a 16-bit process id.
        if bits & 0x0100_0000 != 0 {
            self.skip(2)?;
        }
        Ok(())
    }

    /// Reads a `TNamed` base, returning (name, title).
    pub fn tnamed(&mut self) -> Result<(String, String)> {
        let h = self.class_header()?;
        self.tobject()?;
        let name = self.tstring()?;
        let title = self.tstring()?;
        self.seek(h.end)?;
        Ok((name, title))
    }

    /// Reads a variable-length member array preceded by its one-byte marker.
    pub fn array_i32(&mut self, n: usize) -> Result<Vec<i32>> {
        let _marker = self.u8()?;
        (0..n).map(|_| self.i32()).collect()
    }

    pub fn array_i64(&mut self, n: usize) -> Result<Vec<i64>> {
        let _marker = self.u8()?;
        (0..n).map(|_| self.i64()).collect()
    }

    /// Resolves the class of the next object, following ROOT's tag scheme.
    ///
    /// Returns `None` for a null pointer, otherwise the class name and the
    /// absolute end position of the object.
    pub fn read_object_header(&mut self) -> Result<Option<(String, usize)>> {
        let start = self.pos;
        let bcnt = self.u32()?;
        if bcnt == 0 {
            return Ok(None);
        }

        // ROOT keys the class map on the position of the class tag itself,
        // which follows the byte count when one is present.
        let (tag, tag_pos, end) = if bcnt & K_BYTE_COUNT_MASK == 0 || bcnt == K_NEW_CLASS_TAG {
            (bcnt, start, None)
        } else {
            let end = start + 4 + (bcnt & !K_BYTE_COUNT_MASK) as usize;
            let tag_pos = self.pos;
            (self.u32()?, tag_pos, Some(end))
        };

        if tag & K_CLASS_MASK == 0 {
            // A reference to an object we have already seen; not needed for
            // the containers this reader walks.
            return Err(Error::format(
                "object back-references are not supported by this reader",
            ));
        }

        let classname = if tag == K_NEW_CLASS_TAG {
            let name = self.cstring()?;
            // Record where this class was declared so later tags can refer to it.
            self.classes
                .insert((tag_pos + self.origin) as u32 + K_MAP_OFFSET, name.clone());
            name
        } else {
            let ref_pos = tag & !K_CLASS_MASK;
            self.classes
                .get(&ref_pos)
                .cloned()
                .ok_or_else(|| Error::format(format!("unknown class reference 0x{tag:08x}")))?
        };

        let end = match end {
            Some(e) => e,
            None => {
                // Without a byte count the object's own header supplies one.
                let save = self.pos;
                let h = self.class_header()?;
                self.seek(save)?;
                h.end
            }
        };
        Ok(Some((classname, end)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_big_endian_scalars_and_strings() {
        let mut data = Vec::new();
        data.extend_from_slice(&1234i32.to_be_bytes());
        data.extend_from_slice(&(-2.5f64).to_be_bytes());
        data.push(5);
        data.extend_from_slice(b"hello");

        let mut b = RBuffer::new(&data);
        assert_eq!(b.i32().unwrap(), 1234);
        assert_eq!(b.f64().unwrap(), -2.5);
        assert_eq!(b.tstring().unwrap(), "hello");
        assert_eq!(b.remaining(), 0);
    }

    #[test]
    fn long_tstrings_use_the_32_bit_length() {
        let text = "x".repeat(300);
        let mut data = vec![255u8];
        data.extend_from_slice(&(300u32).to_be_bytes());
        data.extend_from_slice(text.as_bytes());
        let mut b = RBuffer::new(&data);
        assert_eq!(b.tstring().unwrap(), text);
    }

    #[test]
    fn class_header_reports_the_object_end() {
        let mut data = Vec::new();
        data.extend_from_slice(&(10u32 | K_BYTE_COUNT_MASK).to_be_bytes());
        data.extend_from_slice(&3u16.to_be_bytes());
        data.extend_from_slice(&[0u8; 8]);
        let mut b = RBuffer::new(&data);
        let h = b.class_header().unwrap();
        assert_eq!(h.version, 3);
        assert_eq!(h.end, 14);
    }

    #[test]
    fn reads_past_the_end_are_errors_not_panics() {
        let data = [0u8; 2];
        let mut b = RBuffer::new(&data);
        assert!(b.i32().is_err());
    }
}
