//! Serialising a flat `TTree` and its baskets.
//!
//! Only what a flat ntuple needs is modelled: one basket per branch,
//! fixed-width entries (so no entry-offset table) and no sub-branches.
//!
//! Columns are described by [`Branchable`] rather than by the caller, so the
//! ROOT leaf class, the type code in the branch title, the element width and
//! the signedness all follow from the Rust type:
//!
//! ```
//! use root_io::write::tree::TreeWriter;
//!
//! let mut tree = TreeWriter::new("Candidates", "one row per candidate");
//! tree.column("event", &[1u64, 2, 3]);
//! tree.column("mass", &[125.1f32, 91.2, 40.0]);
//! tree.column("muonPt", &[[30.0f32, 20.0, 10.0, 5.0]; 3]);
//! assert_eq!(tree.rows().unwrap(), 3);
//! ```

use crate::wbuffer::WBuffer;
use crate::error::{Error, Result};

/// `ROOT::TIOFeatures` as ROOT writes it — an opaque, fixed 11-byte record
/// (byte count, version and the feature bits, all currently constant).
const IO_FEATURES: [u8; 11] = [
    0x40, 0x00, 0x00, 0x07, 0x00, 0x00, 0x1a, 0xa1, 0x2f, 0x10, 0x00,
];

/// Default basket size ROOT gives a new branch.
const BASKET_SIZE: i32 = 32000;
/// Slots reserved in the basket tables.
const MAX_BASKETS: i32 = 10;

/// The buffer size ROOT records for a basket: its default, unless the payload
/// needs more.
pub fn basket_buffer_size(data_len: usize) -> i32 {
    BASKET_SIZE.max(data_len as i32 + 512)
}

/// The storage type of a leaf: what picks the `TLeaf*` class and the letter in
/// the branch title.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeafType {
    Bool,
    I8,
    U8,
    I16,
    U16,
    I32,
    U32,
    I64,
    U64,
    F32,
    F64,
}

impl LeafType {
    /// The ROOT class written for this type.
    pub fn class(self) -> &'static str {
        match self {
            LeafType::Bool => "TLeafO",
            LeafType::I8 | LeafType::U8 => "TLeafB",
            LeafType::I16 | LeafType::U16 => "TLeafS",
            LeafType::I32 | LeafType::U32 => "TLeafI",
            LeafType::I64 | LeafType::U64 => "TLeafL",
            LeafType::F32 => "TLeafF",
            LeafType::F64 => "TLeafD",
        }
    }

    /// The letter after the slash in a leaf list, as in `mass/F`.
    pub fn code(self) -> char {
        match self {
            LeafType::Bool => 'O',
            LeafType::I8 => 'B',
            LeafType::U8 => 'b',
            LeafType::I16 => 'S',
            LeafType::U16 => 's',
            LeafType::I32 => 'I',
            LeafType::U32 => 'i',
            LeafType::I64 => 'L',
            LeafType::U64 => 'l',
            LeafType::F32 => 'F',
            LeafType::F64 => 'D',
        }
    }

    /// Bytes one element occupies, ROOT's `fLenType`.
    pub fn size(self) -> i32 {
        match self {
            LeafType::Bool | LeafType::I8 | LeafType::U8 => 1,
            LeafType::I16 | LeafType::U16 => 2,
            LeafType::I32 | LeafType::U32 | LeafType::F32 => 4,
            LeafType::I64 | LeafType::U64 | LeafType::F64 => 8,
        }
    }

    pub fn is_unsigned(self) -> bool {
        matches!(
            self,
            LeafType::U8 | LeafType::U16 | LeafType::U32 | LeafType::U64 | LeafType::Bool
        )
    }

    /// The observed minimum and maximum the concrete `TLeaf*` subclass carries.
    /// ROOT only uses them for range-checked leaves and writes zeroes here.
    fn write_range(self, w: &mut WBuffer) {
        match self {
            LeafType::I64 | LeafType::U64 => {
                w.i64(0);
                w.i64(0);
            }
            LeafType::F32 => {
                w.f32(0.0);
                w.f32(0.0);
            }
            LeafType::F64 => {
                w.f64(0.0);
                w.f64(0.0);
            }
            _ => {
                w.i32(0);
                w.i32(0);
            }
        }
    }
}

/// A value that can be stored in a tree column.
///
/// Implemented for the scalar types ROOT has leaves for, and for `[T; N]`,
/// which becomes a fixed-length array branch such as `muonPt[4]/F`.
pub trait Branchable {
    /// The storage type of one element.
    const LEAF: LeafType;
    /// Elements per entry: 1 for a scalar, `N` for `[T; N]`.
    const LEN: i32 = 1;

    /// Appends this value in ROOT's big-endian layout.
    fn encode(&self, out: &mut Vec<u8>);
}

/// Implements [`Branchable`] for a type stored as its big-endian bytes.
macro_rules! impl_branchable {
    ($t:ty, $leaf:ident) => {
        impl Branchable for $t {
            const LEAF: LeafType = LeafType::$leaf;

            fn encode(&self, out: &mut Vec<u8>) {
                out.extend_from_slice(&self.to_be_bytes());
            }
        }
    };
}

impl_branchable!(i8, I8);
impl_branchable!(u8, U8);
impl_branchable!(i16, I16);
impl_branchable!(u16, U16);
impl_branchable!(i32, I32);
impl_branchable!(u32, U32);
impl_branchable!(i64, I64);
impl_branchable!(u64, U64);
impl_branchable!(f32, F32);
impl_branchable!(f64, F64);

impl Branchable for bool {
    const LEAF: LeafType = LeafType::Bool;

    fn encode(&self, out: &mut Vec<u8>) {
        out.push(u8::from(*self));
    }
}

impl<T: Branchable, const N: usize> Branchable for [T; N] {
    const LEAF: LeafType = T::LEAF;
    const LEN: i32 = N as i32;

    fn encode(&self, out: &mut Vec<u8>) {
        for x in self {
            x.encode(out);
        }
    }
}

/// One output branch: its leaf description and the packed entry data.
pub struct OutBranch {
    pub name: String,
    /// The branch title, which is also the leaf list, e.g. `muonPt[4]/F`.
    pub title: String,
    pub leaf: LeafType,
    /// Elements per entry: 1 for a scalar, 4 for a fixed array.
    pub leaf_len: i32,
    /// All entries concatenated, already big-endian.
    pub payload: Vec<u8>,
    /// Rows this column holds, checked against the other columns.
    pub rows: usize,

    // Filled in once the basket has been placed in the file.
    pub basket_seek: i64,
    pub basket_bytes: i32,
    pub tot_bytes: i64,
    pub zip_bytes: i64,
}

impl OutBranch {
    /// Packs a column of values into a branch.
    pub fn new<T: Branchable>(name: &str, values: &[T]) -> Self {
        let mut payload = Vec::with_capacity(values.len() * (T::LEAF.size() * T::LEN) as usize);
        for v in values {
            v.encode(&mut payload);
        }
        let dims = if T::LEN == 1 {
            String::new()
        } else {
            format!("[{}]", T::LEN)
        };
        Self {
            name: name.to_string(),
            title: format!("{name}{dims}/{}", T::LEAF.code()),
            leaf: T::LEAF,
            leaf_len: T::LEN,
            payload,
            rows: values.len(),
            basket_seek: 0,
            basket_bytes: 0,
            tot_bytes: 0,
            zip_bytes: 0,
        }
    }

    /// Bytes each entry occupies.
    pub fn entry_size(&self) -> usize {
        (self.leaf_len * self.leaf.size()) as usize
    }

    fn write_leaf(&self, w: &mut WBuffer) {
        let outer = w.begin_object(self.leaf.class(), 1, Some(&format!("leaf:{}", self.name)));
        let base = w.begin(2); // TLeaf
        w.tnamed(&self.name, &self.name, 0);
        w.i32(self.leaf_len);
        w.i32(self.leaf.size());
        w.i32(0); // fOffset
        w.u8(0); // fIsRange
        w.u8(u8::from(self.leaf.is_unsigned()));
        w.null_ref(); // fLeafCount: fixed-size leaves have none
        w.end(base);
        self.leaf.write_range(w);
        w.end(outer);
    }

    fn write_branch(&self, w: &mut WBuffer, entries: i64) {
        let outer = w.begin_object("TBranch", 13, None);
        w.tnamed(&self.name, &self.title, 0x0040_0000);
        w.tattfill(0, 1001);

        w.i32(101); // fCompress: zlib, level 1
        w.i32(BASKET_SIZE);
        w.i32(0); // fEntryOffsetLen: entries are fixed width
        w.i32(1); // fWriteBasket
        w.i64(entries); // fEntryNumber
        w.raw(&IO_FEATURES);
        w.i32(0); // fOffset
        w.i32(MAX_BASKETS);
        w.i32(0); // fSplitLevel
        w.i64(entries); // fEntries
        w.i64(0); // fFirstEntry
        w.i64(self.tot_bytes);
        w.i64(self.zip_bytes);

        let sub = w.begin_objarray(0); // fBranches
        w.end(sub);

        let leaves = w.begin_objarray(1); // fLeaves
        self.write_leaf(w);
        w.end(leaves);

        let baskets = w.begin_objarray(0); // fBaskets: none held in memory
        w.end(baskets);

        let mut bytes = vec![0i32; MAX_BASKETS as usize];
        bytes[0] = self.basket_bytes;
        let mut entry = vec![0i64; MAX_BASKETS as usize];
        entry[1] = entries;
        let mut seek = vec![0i64; MAX_BASKETS as usize];
        seek[0] = self.basket_seek;
        w.marked_i32(&bytes);
        w.marked_i64(&entry);
        w.marked_i64(&seek);
        w.tstring(""); // fFileName
        w.end(outer);
    }
}

/// A tree being built column by column.
///
/// Every column must hold the same number of rows; [`TreeWriter::rows`]
/// reports it and [`super::RootWriter::write_tree`] refuses a ragged tree.
pub struct TreeWriter {
    pub(crate) name: String,
    pub(crate) title: String,
    pub(crate) branches: Vec<OutBranch>,
}

impl TreeWriter {
    pub fn new(name: &str, title: &str) -> Self {
        Self {
            name: name.to_string(),
            title: title.to_string(),
            branches: Vec::new(),
        }
    }

    /// Adds a column. The branch type follows from `T`.
    ///
    /// ```
    /// # use root_io::write::tree::TreeWriter;
    /// let mut tree = TreeWriter::new("T", "");
    /// tree.column("pt", &[10.0f32, 20.0]);
    /// tree.column("charge", &[1i32, -1]);
    /// ```
    pub fn column<T: Branchable>(&mut self, name: &str, values: &[T]) -> &mut Self {
        self.branches.push(OutBranch::new(name, values));
        self
    }

    /// Adds a column computed from each row of a slice, which saves
    /// materialising a vector per column.
    ///
    /// ```
    /// # use root_io::write::tree::TreeWriter;
    /// struct Candidate { mass: f32, z1: f32 }
    /// let rows = [Candidate { mass: 125.0, z1: 91.0 }];
    /// let mut tree = TreeWriter::new("T", "");
    /// tree.column_with("mass", &rows, |c| c.mass);
    /// tree.column_with("z1Mass", &rows, |c| c.z1);
    /// ```
    pub fn column_with<R, T: Branchable>(
        &mut self,
        name: &str,
        rows: &[R],
        f: impl Fn(&R) -> T,
    ) -> &mut Self {
        let values: Vec<T> = rows.iter().map(f).collect();
        self.column(name, &values)
    }

    /// Column names, in the order they were added.
    pub fn column_names(&self) -> impl Iterator<Item = &str> {
        self.branches.iter().map(|b| b.name.as_str())
    }

    /// The row count every column agrees on, or an error naming the ones that
    /// disagree.
    pub fn rows(&self) -> Result<usize> {
        let mut rows = None;
        for b in &self.branches {
            match rows {
                None => rows = Some(b.rows),
                Some(n) if n != b.rows => {
                    return Err(Error::format(format!(
                        "tree {}: column {} has {} rows but {} has {n}",
                        self.name,
                        b.name,
                        b.rows,
                        self.branches[0].name,
                    )));
                }
                Some(_) => {}
            }
        }
        Ok(rows.unwrap_or(0))
    }
}

/// Serialises the `TTree` object itself.
pub fn serialize_tree(
    name: &str,
    title: &str,
    entries: i64,
    branches: &[OutBranch],
    key_len: usize,
) -> Vec<u8> {
    // Class and object references are numbered from the start of the key.
    let mut w = WBuffer::new(key_len);
    let tree = w.begin(20);
    w.tnamed(name, title, 0x0000_0008);
    w.tattline(602, 1, 1);
    w.tattfill(0, 1001);
    w.tattmarker(1, 1, 1.0);

    let tot: i64 = branches.iter().map(|b| b.tot_bytes).sum();
    let zip: i64 = branches.iter().map(|b| b.zip_bytes).sum();
    w.i64(entries); // fEntries
    w.i64(tot); // fTotBytes
    w.i64(zip); // fZipBytes
    w.i64(0); // fSavedBytes
    w.i64(0); // fFlushedBytes
    w.f64(1.0); // fWeight
    w.i32(0); // fTimerInterval
    w.i32(25); // fScanField
    w.i32(0); // fUpdate
    w.i32(1000); // fDefaultEntryOffsetLen
    w.i32(0); // fNClusterRange
    w.i64(1_000_000_000_000); // fMaxEntries
    w.i64(1_000_000_000_000); // fMaxEntryLoop
    w.i64(0); // fMaxVirtualSize
    w.i64(-300_000_000); // fAutoSave
    w.i64(-30_000_000); // fAutoFlush
    w.i64(1_000_000); // fEstimate
    w.u8(0); // fClusterRangeEnd: empty
    w.u8(0); // fClusterSize: empty
    w.raw(&IO_FEATURES);

    let br = w.begin_objarray(branches.len() as i32);
    for b in branches {
        b.write_branch(&mut w, entries);
    }
    w.end(br);

    // The tree's leaf list points at the leaves already written in the
    // branches rather than repeating them.
    let lv = w.begin_objarray(branches.len() as i32);
    for b in branches {
        w.object_ref(&format!("leaf:{}", b.name))
            .expect("leaf was serialised with its branch");
    }
    w.end(lv);

    w.null_ref(); // fAliases
    w.array_f64(&[]); // fIndexValues
    w.array_i32_member(&[]); // fIndex
    w.null_ref(); // fTreeIndex
    w.null_ref(); // fFriends
    w.null_ref(); // fUserInfo
    w.null_ref(); // fBranchRef
    w.end(tree);
    w.into_vec()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::RBuffer;

    fn sample_branch() -> OutBranch {
        OutBranch::new("run", &[0u32; 10])
    }

    #[test]
    fn the_rust_type_picks_the_leaf_and_the_title() {
        let b = OutBranch::new("muonPt", &[[0.0f32; 4]; 3]);
        assert_eq!(b.title, "muonPt[4]/F");
        assert_eq!(b.leaf.class(), "TLeafF");
        assert_eq!(b.entry_size(), 16);
        assert_eq!(b.rows, 3);

        let b = OutBranch::new("event", &[1u64, 2]);
        assert_eq!(b.title, "event/l");
        assert_eq!(b.leaf.class(), "TLeafL");
        assert!(b.leaf.is_unsigned());
        assert_eq!(b.entry_size(), 8);

        assert_eq!(OutBranch::new("run", &[1u32]).title, "run/i");
        assert_eq!(OutBranch::new("q", &[1i32]).title, "q/I");
        assert_eq!(OutBranch::new("ok", &[true]).title, "ok/O");
        assert_eq!(OutBranch::new("x", &[1.0f64]).title, "x/D");
    }

    #[test]
    fn values_are_packed_big_endian() {
        let b = OutBranch::new("x", &[1.5f32, -2.0]);
        assert_eq!(b.payload, [1.5f32.to_be_bytes(), (-2.0f32).to_be_bytes()].concat());
    }

    #[test]
    fn a_ragged_tree_is_rejected() {
        let mut t = TreeWriter::new("T", "");
        t.column("a", &[1u32, 2, 3]);
        assert_eq!(t.rows().unwrap(), 3);
        t.column("b", &[1.0f32]);
        let err = t.rows().unwrap_err().to_string();
        assert!(err.contains("column b has 1 rows"), "unhelpful: {err}");
    }

    #[test]
    fn an_empty_tree_has_no_rows() {
        assert_eq!(TreeWriter::new("T", "").rows().unwrap(), 0);
    }

    #[test]
    fn entry_size_accounts_for_fixed_arrays() {
        let b = OutBranch::new("muonPt", &[[0.0f32; 4]; 2]);
        assert_eq!(b.entry_size(), 16);
        assert_eq!(sample_branch().entry_size(), 4);
    }

    #[test]
    fn the_serialised_tree_parses_back_with_the_reader() {
        let branches = vec![sample_branch()];
        let bytes = serialize_tree("T", "a tree", 10, &branches, 75);
        let key = crate::file::TKey {
            nbytes: 0,
            version: 4,
            obj_len: bytes.len() as i32,
            key_len: 75,
            cycle: 1,
            seek_key: 0,
            seek_pdir: 0,
            class_name: "TTree".into(),
            name: "T".into(),
            title: "a tree".into(),
        };
        let tree = crate::tree::Tree::parse(&bytes, &key).expect("tree should parse");
        assert_eq!(tree.name, "T");
        assert_eq!(tree.entries, 10);
        assert_eq!(tree.branches.len(), 1);
        assert_eq!(tree.branches[0].name, "run");
        assert_eq!(
            tree.branches[0].leaves[0].kind,
            crate::tree::LeafKind::U32
        );
    }

    #[test]
    fn byte_counts_cover_the_whole_record() {
        let branches = vec![sample_branch()];
        let bytes = serialize_tree("T", "t", 10, &branches, 75);
        let mut r = RBuffer::new(&bytes);
        let h = r.class_header().unwrap();
        assert_eq!(h.version, 20);
        assert_eq!(h.end, bytes.len());
    }
}
