//! Typed columns, the shape a NanoAOD branch arrives in.
//!
//! NanoAOD is columnar: a branch holds one value per event (`run`) or a
//! variable-length list per event (`Muon_pt`). The two are [`Vec<T>`] and
//! [`Jagged<T>`] here, and which one a branch yields is decided by the branch,
//! not by the caller.
//!
//! [`Jagged<T>`] keeps every value in one allocation and indexes into it, so a
//! collection of a million events costs two allocations rather than a million:
//!
//! ```
//! # use root_io::Jagged;
//! let pt: Jagged<f32> = Jagged::from_parts(vec![10.0, 20.0, 5.0], vec![0, 2, 3]);
//! assert_eq!(&pt[0], &[10.0, 20.0]);
//! assert_eq!(&pt[1], &[5.0]);
//! assert_eq!(pt.len(), 2);
//! ```

use std::ops::Index;

use crate::tree::LeafKind;

/// A value that can be decoded from the bytes of a ROOT branch.
///
/// Implemented for the types NanoAOD stores: `bool`, the signed and unsigned
/// integers, `f32` and `f64`. Reading a branch as the wrong type is an error
/// that names both types rather than producing silently wrong numbers.
pub trait Element: Copy + 'static {
    /// How this type is spelled in diagnostics.
    const NAME: &'static str;
    /// Bytes one value occupies on disk.
    const SIZE: usize;

    /// Whether a branch of `kind` may be read as this type.
    ///
    /// Signed and unsigned integers of the same width are interchangeable,
    /// matching how ROOT itself treats them.
    fn accepts(kind: LeafKind) -> bool;

    /// Decodes one value from exactly [`Element::SIZE`] big-endian bytes.
    fn decode(bytes: &[u8]) -> Self;
}

/// Implements [`Element`] for a type stored as big-endian bytes.
macro_rules! impl_element {
    ($t:ty, $size:expr, $($kind:ident)|+) => {
        impl Element for $t {
            const NAME: &'static str = stringify!($t);
            const SIZE: usize = $size;

            fn accepts(kind: LeafKind) -> bool {
                matches!(kind, $(LeafKind::$kind)|+)
            }

            fn decode(bytes: &[u8]) -> Self {
                // The callers below always hand over `SIZE` bytes, taken from
                // `chunks_exact`, so this conversion cannot fail.
                let raw = bytes.try_into().expect("decode called with SIZE bytes");
                <$t>::from_be_bytes(raw)
            }
        }
    };
}

impl_element!(i8, 1, I8 | U8);
impl_element!(u8, 1, I8 | U8);
impl_element!(i16, 2, I16 | U16);
impl_element!(u16, 2, I16 | U16);
impl_element!(i32, 4, I32 | U32);
impl_element!(u32, 4, I32 | U32);
impl_element!(i64, 8, I64 | U64);
impl_element!(u64, 8, I64 | U64);
impl_element!(f32, 4, F32);
impl_element!(f64, 8, F64);

impl Element for bool {
    const NAME: &'static str = "bool";
    const SIZE: usize = 1;

    fn accepts(kind: LeafKind) -> bool {
        matches!(kind, LeafKind::Bool | LeafKind::I8 | LeafKind::U8)
    }

    fn decode(bytes: &[u8]) -> Self {
        bytes[0] != 0
    }
}

/// A variable-length list per event: NanoAOD's `Muon_pt`, `Jet_eta` and the
/// like, and the shape a whole collection takes once its fields are zipped.
///
/// Indexing yields the slice for one event, so `&muons[i]` is a `&[Muon]` and
/// iteration walks the events.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Jagged<T> {
    values: Vec<T>,
    /// `len() + 1` boundaries into `values`; always starts at 0.
    offsets: Vec<usize>,
}

impl<T> Jagged<T> {
    /// Builds a column from a flat value buffer and its `len + 1` boundaries.
    ///
    /// # Panics
    /// If `offsets` is empty, does not start at zero, or does not end at
    /// `values.len()`.
    pub fn from_parts(values: Vec<T>, offsets: Vec<usize>) -> Self {
        assert_eq!(offsets.first().copied(), Some(0), "offsets must start at 0");
        assert_eq!(
            offsets.last().copied(),
            Some(values.len()),
            "offsets must end at the value count"
        );
        Self { values, offsets }
    }

    /// Number of events.
    pub fn len(&self) -> usize {
        self.offsets.len() - 1
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The list for `event`, or `None` if it is past the end.
    pub fn get(&self, event: usize) -> Option<&[T]> {
        let &start = self.offsets.get(event)?;
        let &end = self.offsets.get(event + 1)?;
        Some(&self.values[start..end])
    }

    /// Every value across every event, back to back.
    pub fn values(&self) -> &[T] {
        &self.values
    }

    /// The lists, in event order.
    pub fn iter(&self) -> impl Iterator<Item = &[T]> {
        self.offsets
            .windows(2)
            .map(move |w| &self.values[w[0]..w[1]])
    }

    /// The `len() + 1` event boundaries into [`Jagged::values`].
    ///
    /// Two columns of the same collection always have equal boundaries, which
    /// is what [`nano_object!`](crate::nano_object) checks when it zips the
    /// fields of a collection together.
    pub fn offsets(&self) -> &[usize] {
        &self.offsets
    }
}

impl<T> Index<usize> for Jagged<T> {
    type Output = [T];

    fn index(&self, event: usize) -> &[T] {
        let start = self.offsets[event];
        let end = self.offsets[event + 1];
        &self.values[start..end]
    }
}

impl<'a, T> IntoIterator for &'a Jagged<T> {
    type Item = &'a [T];
    type IntoIter = Box<dyn Iterator<Item = &'a [T]> + 'a>;

    fn into_iter(self) -> Self::IntoIter {
        Box::new(self.iter())
    }
}

impl<T> FromIterator<Vec<T>> for Jagged<T> {
    fn from_iter<I: IntoIterator<Item = Vec<T>>>(iter: I) -> Self {
        let mut values = Vec::new();
        let mut offsets = vec![0];
        for event in iter {
            values.extend(event);
            offsets.push(values.len());
        }
        Self { values, offsets }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indexes_and_iterates_by_event() {
        let j: Jagged<i32> = vec![vec![1, 2], vec![], vec![3]].into_iter().collect();
        assert_eq!(j.len(), 3);
        assert_eq!(&j[0], &[1, 2]);
        assert_eq!(&j[1], &[] as &[i32]);
        assert_eq!(&j[2], &[3]);
        assert_eq!(j.get(3), None);
        assert_eq!(j.iter().count(), 3);
        assert_eq!(j.values(), &[1, 2, 3]);
    }

    #[test]
    fn integers_accept_either_signedness_of_their_width() {
        assert!(<i32 as Element>::accepts(LeafKind::U32));
        assert!(<u32 as Element>::accepts(LeafKind::I32));
        assert!(!<i32 as Element>::accepts(LeafKind::F32));
        assert!(!<f32 as Element>::accepts(LeafKind::F64));
        assert!(<bool as Element>::accepts(LeafKind::Bool));
    }

    #[test]
    fn decodes_big_endian_values() {
        assert_eq!(<f32 as Element>::decode(&1.5f32.to_be_bytes()), 1.5);
        assert_eq!(<i32 as Element>::decode(&(-7i32).to_be_bytes()), -7);
        assert_eq!(<u64 as Element>::decode(&9u64.to_be_bytes()), 9);
        assert!(<bool as Element>::decode(&[1]));
        assert!(!<bool as Element>::decode(&[0]));
    }
}
