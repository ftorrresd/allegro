//! `TTree` metadata and basket decoding.
//!
//! The member layouts below follow the streamer descriptions stored in the
//! file itself (TTree v20, TBranch v13, TLeaf v2), and each parse is checked
//! against the object's byte count so a layout mismatch fails loudly instead
//! of silently producing wrong numbers.

use crate::buffer::RBuffer;
use crate::compress;
use crate::file::TKey;
use crate::source::ReadAt;
use crate::error::{Error, Result};

/// The element type a leaf holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeafKind {
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

impl LeafKind {
    pub fn size(self) -> usize {
        match self {
            LeafKind::Bool | LeafKind::I8 | LeafKind::U8 => 1,
            LeafKind::I16 | LeafKind::U16 => 2,
            LeafKind::I32 | LeafKind::U32 | LeafKind::F32 => 4,
            LeafKind::I64 | LeafKind::U64 | LeafKind::F64 => 8,
        }
    }

    /// The Rust type this leaf should be read as, for diagnostics.
    pub fn type_name(self) -> &'static str {
        match self {
            LeafKind::Bool => "bool",
            LeafKind::I8 => "i8",
            LeafKind::U8 => "u8",
            LeafKind::I16 => "i16",
            LeafKind::U16 => "u16",
            LeafKind::I32 => "i32",
            LeafKind::U32 => "u32",
            LeafKind::I64 => "i64",
            LeafKind::U64 => "u64",
            LeafKind::F32 => "f32",
            LeafKind::F64 => "f64",
        }
    }

    fn from_class(class: &str, unsigned: bool) -> Result<Self> {
        Ok(match class {
            "TLeafO" => LeafKind::Bool,
            "TLeafB" => {
                if unsigned {
                    LeafKind::U8
                } else {
                    LeafKind::I8
                }
            }
            "TLeafS" => {
                if unsigned {
                    LeafKind::U16
                } else {
                    LeafKind::I16
                }
            }
            "TLeafI" => {
                if unsigned {
                    LeafKind::U32
                } else {
                    LeafKind::I32
                }
            }
            "TLeafL" => {
                if unsigned {
                    LeafKind::U64
                } else {
                    LeafKind::I64
                }
            }
            "TLeafF" => LeafKind::F32,
            "TLeafD" => LeafKind::F64,
            other => return Err(Error::format(format!("unsupported leaf class {other}"))),
        })
    }
}

#[derive(Debug, Clone)]
pub struct Leaf {
    pub name: String,
    pub kind: LeafKind,
    /// Fixed element count; 1 for a scalar.
    pub len: i32,
    /// Whether the length varies per entry, driven by a counter leaf.
    pub has_count: bool,
}

#[derive(Debug, Clone)]
pub struct Branch {
    pub name: String,
    pub entries: i64,
    pub entry_offset_len: i32,
    pub write_basket: i32,
    pub basket_bytes: Vec<i32>,
    pub basket_entry: Vec<i64>,
    pub basket_seek: Vec<i64>,
    pub leaves: Vec<Leaf>,
}

impl Branch {
    pub fn leaf(&self) -> Result<&Leaf> {
        self.leaves
            .first()
            .ok_or_else(|| Error::format(format!("branch {} has no leaf", self.name)))
    }
}

#[derive(Debug, Clone)]
pub struct Tree {
    pub name: String,
    pub title: String,
    pub entries: i64,
    pub branches: Vec<Branch>,
}

impl Tree {
    pub fn branch(&self, name: &str) -> Result<&Branch> {
        self.branches
            .iter()
            .find(|b| b.name == name)
            .ok_or_else(|| Error::format(format!("no branch named {name} in tree {}", self.name)))
    }

    pub fn branch_names(&self) -> Vec<&str> {
        self.branches.iter().map(|b| b.name.as_str()).collect()
    }

    /// Parses a `TTree` from the decompressed payload of its key.
    pub fn parse(payload: &[u8], key: &TKey) -> Result<Self> {
        let mut b = RBuffer::with_origin(payload, key.key_len.max(0) as usize);
        let h = b.class_header()?;
        if h.version != 20 {
            return Err(Error::format(format!(
                "TTree version {} is not supported (expected 20)",
                h.version
            )));
        }
        let (name, title) = b.tnamed()?;
        // TAttLine, TAttFill, TAttMarker
        b.skip_object()?;
        b.skip_object()?;
        b.skip_object()?;

        let entries = b.i64()?;
        let _tot_bytes = b.i64()?;
        let _zip_bytes = b.i64()?;
        let _saved_bytes = b.i64()?;
        let _flushed_bytes = b.i64()?;
        let _weight = b.f64()?;
        let _timer_interval = b.i32()?;
        let _scan_field = b.i32()?;
        let _update = b.i32()?;
        let _default_entry_offset_len = b.i32()?;
        let n_cluster_range = b.i32()?;
        let _max_entries = b.i64()?;
        let _max_entry_loop = b.i64()?;
        let _max_virtual_size = b.i64()?;
        let _auto_save = b.i64()?;
        let _auto_flush = b.i64()?;
        let _estimate = b.i64()?;
        let _cluster_range_end = b.array_i64(n_cluster_range as usize)?;
        let _cluster_size = b.array_i64(n_cluster_range as usize)?;
        b.skip_object()?; // ROOT::TIOFeatures

        let branches = read_branch_array(&mut b)?;

        Ok(Self {
            name: if name.is_empty() { key.name.clone() } else { name },
            title,
            entries,
            branches,
        })
    }
}

/// Reads a `TObjArray` of `TBranch` objects.
fn read_branch_array(b: &mut RBuffer) -> Result<Vec<Branch>> {
    let h = b.class_header()?;
    b.tobject()?;
    let _name = b.tstring()?;
    let nobjects = b.i32()?;
    let _lower_bound = b.i32()?;

    let mut out = Vec::with_capacity(nobjects.max(0) as usize);
    for i in 0..nobjects.max(0) {
        let Some((class, end)) = b.read_object_header()? else {
            continue;
        };
        if class != "TBranch" {
            // TBranchElement and friends describe split objects, which flat
            // NanoAOD trees do not use.
            return Err(Error::format(format!(
                "branch {i} has unsupported class {class}"
            )));
        }
        out.push(parse_branch(b)?);
        b.seek(end)?;
    }
    b.seek(h.end)?;
    Ok(out)
}

fn parse_branch(b: &mut RBuffer) -> Result<Branch> {
    let h = b.class_header()?;
    if h.version != 13 {
        return Err(Error::format(format!(
            "TBranch version {} is not supported (expected 13)",
            h.version
        )));
    }
    let (name, _title) = b.tnamed()?;
    b.skip_object()?; // TAttFill

    let _compress = b.i32()?;
    let _basket_size = b.i32()?;
    let entry_offset_len = b.i32()?;
    let write_basket = b.i32()?;
    let _entry_number = b.i64()?;
    b.skip_object()?; // ROOT::TIOFeatures
    let _offset = b.i32()?;
    let max_baskets = b.i32()?;
    let _split_level = b.i32()?;
    let entries = b.i64()?;
    let _first_entry = b.i64()?;
    let _tot_bytes = b.i64()?;
    let _zip_bytes = b.i64()?;

    // Sub-branches are always empty for a flat tree, but the array is present.
    let _sub = read_branch_array(b)?;
    let leaves = read_leaf_array(b)?;
    skip_object_array(b)?; // fBaskets: in-memory only, written empty

    let n = max_baskets.max(0) as usize;
    let basket_bytes = b.array_i32(n)?;
    let basket_entry = b.array_i64(n)?;
    let basket_seek = b.array_i64(n)?;
    let _file_name = b.tstring()?;

    if b.pos() != h.end {
        return Err(Error::format(format!(
            "TBranch {name}: parsed {} bytes but the record declares {}",
            b.pos(),
            h.end
        )));
    }

    Ok(Branch {
        name,
        entries,
        entry_offset_len,
        write_basket,
        basket_bytes,
        basket_entry,
        basket_seek,
        leaves,
    })
}

fn read_leaf_array(b: &mut RBuffer) -> Result<Vec<Leaf>> {
    let h = b.class_header()?;
    b.tobject()?;
    let _name = b.tstring()?;
    let nobjects = b.i32()?;
    let _lower_bound = b.i32()?;

    let mut out = Vec::new();
    for _ in 0..nobjects.max(0) {
        let Some((class, end)) = b.read_object_header()? else {
            continue;
        };
        out.push(parse_leaf(b, &class)?);
        b.seek(end)?;
    }
    b.seek(h.end)?;
    Ok(out)
}

fn parse_leaf(b: &mut RBuffer, class: &str) -> Result<Leaf> {
    let outer = b.class_header()?;
    // BASE TLeaf
    let base = b.class_header()?;
    if base.version != 2 {
        return Err(Error::format(format!(
            "TLeaf version {} is not supported (expected 2)",
            base.version
        )));
    }
    let (name, _title) = b.tnamed()?;
    let len = b.i32()?;
    let _len_type = b.i32()?;
    let _offset = b.i32()?;
    let _is_range = b.u8()?;
    let is_unsigned = b.u8()? != 0;
    // fLeafCount is a pointer to the counter leaf. Only its presence matters
    // here, and reading it as a raw word avoids resolving the back-reference.
    let count_tag = b.u32()?;
    let has_count = count_tag != 0;

    b.seek(base.end)?;
    b.seek(outer.end)?;

    Ok(Leaf {
        name,
        kind: LeafKind::from_class(class, is_unsigned)?,
        len,
        has_count,
    })
}

/// Skips a `TObjArray` without interpreting its contents.
fn skip_object_array(b: &mut RBuffer) -> Result<()> {
    let h = b.class_header()?;
    b.seek(h.end)
}

/// One decoded basket: the payload plus per-entry byte boundaries.
pub struct Basket {
    pub data: Vec<u8>,
    /// `entries + 1` boundaries into `data`; empty when entries are fixed size.
    pub offsets: Vec<i32>,
    pub n_entries: i32,
}

/// Reads and decodes basket `index` of `branch`.
pub fn read_basket(src: &mut dyn ReadAt, branch: &Branch, index: usize) -> Result<Basket> {
    let seek = *branch
        .basket_seek
        .get(index)
        .ok_or_else(|| Error::format(format!("basket {index} out of range")))? as u64;
    let nbytes = branch.basket_bytes[index] as usize;
    if seek == 0 || nbytes == 0 {
        return Err(Error::format(format!(
            "branch {} basket {index} is not present in the file",
            branch.name
        )));
    }

    let raw = src.read_at(seek, nbytes)?;
    let mut b = RBuffer::new(&raw);
    let key = TKey::read(&mut b)?;

    // The basket header follows the key, starting with TBasket's own version.
    let _basket_version = b.u16()?;
    let _buffer_size = b.i32()?;
    let nev_buf_size = b.i32()?;
    let n_entries = b.i32()?;
    let last = b.i32()?;
    let _flag = b.u8()?;

    let payload = &raw[key.key_len as usize..];
    let object = if key.is_compressed() {
        compress::decompress(payload, key.obj_len as usize)?
    } else {
        payload.to_vec()
    };

    // `fLast` is measured from the start of the key, so shift it into the
    // object's own coordinates to find where the data ends.
    let border = (last - key.key_len as i32).max(0) as usize;
    if border > object.len() {
        return Err(Error::format(format!(
            "branch {} basket {index}: data border {border} exceeds {} bytes",
            branch.name,
            object.len()
        )));
    }

    let mut offsets = Vec::new();
    if nev_buf_size > 8 && object.len() > border {
        // The trailing region holds a leading count word followed by the
        // absolute start offset of each entry.
        let tail = &object[border..];
        let mut tb = RBuffer::new(tail);
        let _count = tb.i32()?;
        offsets.reserve(n_entries as usize + 1);
        for _ in 0..n_entries {
            offsets.push(tb.i32()? - key.key_len as i32);
        }
        offsets.push(border as i32);
    }

    Ok(Basket {
        data: object[..border].to_vec(),
        offsets,
        n_entries,
    })
}

/// Every entry of a branch, concatenated into one buffer.
///
/// Entries are delimited by `offsets` rather than held in a vector each, so
/// reading a branch costs two allocations however many events the file has.
pub struct RawColumn {
    pub data: Vec<u8>,
    /// `entries + 1` boundaries into `data`, starting at 0.
    pub offsets: Vec<usize>,
}

impl RawColumn {
    /// Number of entries.
    pub fn len(&self) -> usize {
        self.offsets.len() - 1
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The bytes of one entry.
    pub fn entry(&self, index: usize) -> &[u8] {
        &self.data[self.offsets[index]..self.offsets[index + 1]]
    }
}

/// Reads every entry of `branch`, in entry order.
pub fn read_column(src: &mut dyn ReadAt, branch: &Branch) -> Result<RawColumn> {
    let entries = branch.entries.max(0) as usize;
    let mut data = Vec::new();
    let mut offsets = Vec::with_capacity(entries + 1);
    offsets.push(0);

    for i in 0..branch.write_basket.max(0) as usize {
        let basket = read_basket(src, branch, i)?;
        let n = basket.n_entries.max(0) as usize;

        if basket.offsets.is_empty() {
            // Fixed-size entries: the payload splits evenly.
            let leaf = branch.leaf()?;
            let width = leaf.kind.size() * leaf.len.max(1) as usize;
            if width == 0 || basket.data.len() < width * n {
                return Err(Error::format(format!(
                    "branch {} basket {i}: {} bytes cannot hold {n} entries of {width}",
                    branch.name,
                    basket.data.len(),
                )));
            }
            offsets.extend((1..=n).map(|e| data.len() + e * width));
            data.extend_from_slice(&basket.data[..n * width]);
        } else {
            for e in 0..n {
                let start = basket.offsets[e].max(0) as usize;
                let end = basket.offsets[e + 1].max(0) as usize;
                if start > end || end > basket.data.len() {
                    return Err(Error::format(format!(
                        "branch {} basket {i}: entry {e} spans {start}..{end} of {} bytes",
                        branch.name,
                        basket.data.len()
                    )));
                }
                data.extend_from_slice(&basket.data[start..end]);
                offsets.push(data.len());
            }
        }
    }

    let column = RawColumn { data, offsets };
    if column.len() as i64 != branch.entries {
        return Err(Error::format(format!(
            "branch {} yielded {} entries but declares {}",
            branch.name,
            column.len(),
            branch.entries
        )));
    }
    Ok(column)
}
