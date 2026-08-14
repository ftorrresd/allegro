//! Serialisation buffer for ROOT records.
//!
//! Mirrors [`crate::buffer::RBuffer`]: values are big-endian and
//! composite objects carry a byte count that is patched once their extent is
//! known. Polymorphic members additionally carry a class tag, which ROOT
//! writes in full only the first time a class appears and abbreviates
//! afterwards as an offset back to that first occurrence. Objects referenced
//! from more than one container (leaves, which live in both the branch and the
//! tree) are likewise written once and referenced by offset.

use std::collections::HashMap;

use crate::error::{Error, Result};

const K_BYTE_COUNT_MASK: u32 = 0x4000_0000;
/// Marks a tag as naming a class rather than referring to an object.
const K_CLASS_MASK: u32 = 0x8000_0000;
const K_NEW_CLASS_TAG: u32 = 0xFFFF_FFFF;
const K_MAP_OFFSET: u32 = 2;

/// Marks an object whose byte count is still to be patched.
#[derive(Debug, Clone, Copy)]
pub struct Marker {
    outer: Option<usize>,
    inner: usize,
}

pub struct WBuffer {
    data: Vec<u8>,
    /// Displacement of `data[0]` in ROOT's coordinates: the length of the key
    /// that will precede this payload.
    origin: usize,
    classes: HashMap<String, u32>,
    objects: HashMap<String, u32>,
}

impl WBuffer {
    pub fn new(origin: usize) -> Self {
        Self {
            data: Vec::new(),
            origin,
            classes: HashMap::new(),
            objects: HashMap::new(),
        }
    }

    pub fn pos(&self) -> usize {
        self.data.len()
    }

    pub fn into_vec(self) -> Vec<u8> {
        self.data
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.data
    }

    pub fn u8(&mut self, v: u8) {
        self.data.push(v);
    }
    pub fn i16(&mut self, v: i16) {
        self.data.extend_from_slice(&v.to_be_bytes());
    }
    pub fn u16(&mut self, v: u16) {
        self.data.extend_from_slice(&v.to_be_bytes());
    }
    pub fn i32(&mut self, v: i32) {
        self.data.extend_from_slice(&v.to_be_bytes());
    }
    pub fn u32(&mut self, v: u32) {
        self.data.extend_from_slice(&v.to_be_bytes());
    }
    pub fn i64(&mut self, v: i64) {
        self.data.extend_from_slice(&v.to_be_bytes());
    }
    pub fn u64(&mut self, v: u64) {
        self.data.extend_from_slice(&v.to_be_bytes());
    }
    pub fn f32(&mut self, v: f32) {
        self.data.extend_from_slice(&v.to_be_bytes());
    }
    pub fn f64(&mut self, v: f64) {
        self.data.extend_from_slice(&v.to_be_bytes());
    }
    pub fn raw(&mut self, v: &[u8]) {
        self.data.extend_from_slice(v);
    }

    pub fn tstring(&mut self, s: &str) {
        let b = s.as_bytes();
        if b.len() < 255 {
            self.u8(b.len() as u8);
        } else {
            self.u8(255);
            self.u32(b.len() as u32);
        }
        self.raw(b);
    }

    pub fn cstring(&mut self, s: &str) {
        self.raw(s.as_bytes());
        self.u8(0);
    }

    /// Starts a plain object: byte-count placeholder followed by its version.
    pub fn begin(&mut self, version: u16) -> Marker {
        let inner = self.data.len();
        self.u32(0);
        self.u16(version);
        Marker { outer: None, inner }
    }

    /// Starts a polymorphic member: an outer byte count, the class tag, then
    /// the object's own byte count and version.
    pub fn begin_object(&mut self, class: &str, version: u16, object_id: Option<&str>) -> Marker {
        let outer = self.data.len();
        self.u32(0);

        let tag_pos = self.data.len();
        match self.classes.get(class) {
            // A repeated class is named by offset, flagged with kClassMask so
            // the reader can tell it from an object reference.
            Some(&reference) => self.u32(reference | K_CLASS_MASK),
            None => {
                self.u32(K_NEW_CLASS_TAG);
                self.cstring(class);
                // Later occurrences point at the class tag itself.
                self.classes
                    .insert(class.to_string(), (tag_pos + self.origin) as u32 + K_MAP_OFFSET);
            }
        }
        if let Some(id) = object_id {
            // References to the object point at its outer byte count.
            self.objects
                .insert(id.to_string(), (outer + self.origin) as u32 + K_MAP_OFFSET);
        }

        let inner = self.data.len();
        self.u32(0);
        self.u16(version);
        Marker {
            outer: Some(outer),
            inner,
        }
    }

    /// Writes a reference to an object already serialised in this buffer.
    pub fn object_ref(&mut self, object_id: &str) -> Result<()> {
        let reference = *self
            .objects
            .get(object_id)
            .ok_or_else(|| Error::format(format!("no serialised object named {object_id}")))?;
        self.u32(reference);
        Ok(())
    }

    /// A null pointer member.
    pub fn null_ref(&mut self) {
        self.u32(0);
    }

    /// Patches the byte counts opened by `begin`/`begin_object`.
    pub fn end(&mut self, m: Marker) {
        let end = self.data.len();
        let inner_count = (end - m.inner - 4) as u32 | K_BYTE_COUNT_MASK;
        self.data[m.inner..m.inner + 4].copy_from_slice(&inner_count.to_be_bytes());
        if let Some(outer) = m.outer {
            let outer_count = (end - outer - 4) as u32 | K_BYTE_COUNT_MASK;
            self.data[outer..outer + 4].copy_from_slice(&outer_count.to_be_bytes());
        }
    }

    /// The `TObject` base.
    pub fn tobject(&mut self, bits: u32) {
        self.u16(1);
        self.u32(0);
        self.u32(bits);
    }

    /// A complete `TNamed` base.
    pub fn tnamed(&mut self, name: &str, title: &str, bits: u32) {
        let m = self.begin(1);
        self.tobject(bits);
        self.tstring(name);
        self.tstring(title);
        self.end(m);
    }

    /// `TAttLine` with the defaults ROOT gives a freshly created histogram.
    pub fn tattline(&mut self, color: i16, style: i16, width: i16) {
        let m = self.begin(2);
        self.i16(color);
        self.i16(style);
        self.i16(width);
        self.end(m);
    }

    pub fn tattfill(&mut self, color: i16, style: i16) {
        let m = self.begin(2);
        self.i16(color);
        self.i16(style);
        self.end(m);
    }

    pub fn tattmarker(&mut self, color: i16, style: i16, size: f32) {
        let m = self.begin(3);
        self.i16(color);
        self.i16(style);
        self.f32(size);
        self.end(m);
    }

    /// An empty `TList`.
    pub fn empty_tlist(&mut self) {
        let m = self.begin(5);
        self.tobject(0);
        self.tstring("");
        self.i32(0);
        self.end(m);
    }

    /// An empty `TObjArray`; `nobjects` null slots are written when asked.
    pub fn begin_objarray(&mut self, nobjects: i32) -> Marker {
        let m = self.begin(3);
        self.tobject(0);
        self.tstring("");
        self.i32(nobjects);
        self.i32(0);
        m
    }

    /// A `TArrayD`/`TArrayI` member: a count followed by the elements.
    pub fn array_f64(&mut self, v: &[f64]) {
        self.i32(v.len() as i32);
        for x in v {
            self.f64(*x);
        }
    }

    pub fn array_i32_member(&mut self, v: &[i32]) {
        self.i32(v.len() as i32);
        for x in v {
            self.i32(*x);
        }
    }

    /// A variable-size member array, preceded by ROOT's one-byte marker.
    pub fn marked_i32(&mut self, v: &[i32]) {
        self.u8(1);
        for x in v {
            self.i32(*x);
        }
    }

    pub fn marked_i64(&mut self, v: &[i64]) {
        self.u8(1);
        for x in v {
            self.i64(*x);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::RBuffer;

    #[test]
    fn byte_counts_are_patched_to_the_object_extent() {
        let mut w = WBuffer::new(0);
        let m = w.begin(7);
        w.i32(42);
        w.f64(1.5);
        w.end(m);
        let bytes = w.into_vec();

        let mut r = RBuffer::new(&bytes);
        let h = r.class_header().unwrap();
        assert_eq!(h.version, 7);
        assert_eq!(h.end, bytes.len());
        assert_eq!(r.i32().unwrap(), 42);
        assert_eq!(r.f64().unwrap(), 1.5);
    }

    #[test]
    fn tnamed_round_trips_through_the_reader() {
        let mut w = WBuffer::new(0);
        w.tnamed("hello", "a title", 8);
        let bytes = w.into_vec();
        let mut r = RBuffer::new(&bytes);
        assert_eq!(
            r.tnamed().unwrap(),
            ("hello".to_string(), "a title".to_string())
        );
    }

    #[test]
    fn repeated_classes_are_written_once_then_referenced() {
        let mut w = WBuffer::new(0);
        let a = w.begin_object("TLeafI", 1, Some("leaf0"));
        w.i32(1);
        w.end(a);
        let first_len = w.pos();
        let b = w.begin_object("TLeafI", 1, Some("leaf1"));
        w.i32(2);
        w.end(b);
        // The second occurrence omits the 7-byte class name plus its NUL.
        assert!(w.pos() - first_len < first_len);
    }

    #[test]
    fn object_references_resolve_to_recorded_offsets() {
        let mut w = WBuffer::new(75);
        let m = w.begin_object("TLeafI", 1, Some("leaf0"));
        w.i32(0);
        w.end(m);
        assert!(w.object_ref("leaf0").is_ok());
        assert!(w.object_ref("missing").is_err());
    }
}
