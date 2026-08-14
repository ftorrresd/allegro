//! Reading and writing ROOT files, locally or over XRootD.
//!
//! ```no_run
//! use root_io::RootFile;
//!
//! let mut f = RootFile::open("root://host//store/file.root")?;
//! let tree = f.tree("Events")?;
//! let pt = f.jagged::<f32>(&tree, "Muon_pt")?;
//! # Ok::<(), root_io::Error>(())
//! ```
//!
//! Only the records actually asked for are read: opening a file fetches its
//! header and key list, and reading a branch fetches that branch's baskets.
//! Over XRootD each of those becomes a ranged request rather than a download
//! of the whole file.
//!
//! Writing goes through [`write::RootWriter`], which produces a file ROOT
//! itself can open:
//!
//! ```no_run
//! use root_io::write::{tree::TreeWriter, RootWriter};
//!
//! let mut out = RootWriter::create("out.root");
//! let mut t = TreeWriter::new("Events", "");
//! t.column("pt", &[10.0f32, 20.0]);
//! out.write_tree(t)?;
//! out.finish()
//! # ; Ok::<(), root_io::Error>(())
//! ```

pub mod buffer;
pub mod column;
pub mod compress;
pub mod error;
pub mod file;
pub mod source;
pub mod tree;
pub mod wbuffer;
pub mod write;

pub use column::{Element, Jagged};
pub use error::{Context, Error, Result};
pub use file::TKey;
pub use tree::Tree;

use file::FileHeader;
use source::{CachedSource, LocalSource, ReadAt, XrdSource};
use tree::RawColumn;

/// Blocks fetched per cache miss; large enough to amortise network latency,
/// small enough not to pull in data that will never be used.
const CACHE_BLOCK: usize = 256 * 1024;
const CACHE_LIMIT: usize = 64 * 1024 * 1024;

/// Whether a location names a remote file this crate should reach over XRootD.
pub fn is_xrootd_url(path: &str) -> bool {
    path.starts_with("root://") || path.starts_with("roots://")
}

/// An open ROOT file.
///
/// [`RootFile::open`] accepts either a filesystem path or a `root://` URL and
/// picks the transport accordingly; everything downstream is identical.
pub struct RootFile {
    source: Box<dyn ReadAt>,
    header: FileHeader,
    keys: Vec<TKey>,
}

impl RootFile {
    /// Opens a local path or a `root://` URL.
    pub fn open(path: &str) -> Result<Self> {
        let mut source: Box<dyn ReadAt> = if is_xrootd_url(path) {
            Box::new(CachedSource::new(
                XrdSource::open(path)?,
                CACHE_BLOCK,
                CACHE_LIMIT,
            ))
        } else {
            Box::new(CachedSource::new(
                LocalSource::open(path)?,
                CACHE_BLOCK,
                CACHE_LIMIT,
            ))
        };

        let header = FileHeader::read(source.as_mut())?;
        let keys = file::read_top_level_keys(source.as_mut(), &header)?;
        Ok(Self {
            source,
            header,
            keys,
        })
    }

    pub fn describe(&self) -> String {
        self.source.describe()
    }

    pub fn size(&self) -> u64 {
        self.source.size()
    }

    /// Names and classes of the top-level objects.
    pub fn keys(&self) -> &[TKey] {
        &self.keys
    }

    /// The newest cycle of the named top-level object.
    pub fn key(&self, name: &str) -> Result<TKey> {
        self.keys
            .iter()
            .filter(|k| k.name == name)
            .max_by_key(|k| k.cycle)
            .ok_or_else(|| {
                Error::format(format!(
                    "no object named {name} in {}",
                    self.source.describe()
                ))
            })
            .cloned()
    }

    /// Reads the named `TTree`.
    pub fn tree(&mut self, name: &str) -> Result<Tree> {
        let key = self.key(name)?;
        if key.class_name != "TTree" {
            return Err(Error::format(format!(
                "{name} is a {}, not a TTree",
                key.class_name
            )));
        }
        let payload = file::read_object(self.source.as_mut(), &key)?;
        Tree::parse(&payload, &key)
    }

    /// Reads a named object as its decompressed payload, together with the key
    /// that describes it.
    ///
    /// This is the seam a crate that models a ROOT class parses from; the
    /// `histogram` crate reads a `TH1` this way.
    pub fn object(&mut self, name: &str) -> Result<(TKey, Vec<u8>)> {
        let key = self.key(name)?;
        let payload = file::read_object(self.source.as_mut(), &key)?;
        Ok((key, payload))
    }

    /// Reads a branch as raw bytes, checking that `T` matches how it is stored.
    fn column_of<T: Element>(&mut self, tree: &Tree, branch: &str) -> Result<RawColumn> {
        let b = tree.branch(branch)?;
        let kind = b.leaf()?.kind;
        if !T::accepts(kind) {
            return Err(Error::format(format!(
                "branch {branch} holds {}, which cannot be read as {}",
                kind.type_name(),
                T::NAME
            )));
        }
        let b = b.clone();
        tree::read_column(self.source.as_mut(), &b)
    }

    /// Reads a branch holding one value per event, such as `run` or a trigger
    /// decision.
    ///
    /// ```no_run
    /// # use root_io::RootFile;
    /// # let mut f = RootFile::open("f.root")?;
    /// # let tree = f.tree("Events")?;
    /// let run: Vec<u32> = f.scalar(&tree, "run")?;
    /// # Ok::<(), root_io::Error>(())
    /// ```
    pub fn scalar<T: Element>(&mut self, tree: &Tree, branch: &str) -> Result<Vec<T>> {
        let raw = self.column_of::<T>(tree, branch)?;
        (0..raw.len())
            .map(|e| {
                let bytes = raw.entry(e);
                if bytes.len() < T::SIZE {
                    return Err(Error::format(format!(
                        "entry {e} of {branch} holds {} bytes, too few for {}",
                        bytes.len(),
                        T::NAME
                    )));
                }
                Ok(T::decode(&bytes[..T::SIZE]))
            })
            .collect()
    }

    /// Reads a branch holding a variable-length list per event, such as
    /// `Muon_pt`.
    ///
    /// ```no_run
    /// # use root_io::RootFile;
    /// # let mut f = RootFile::open("f.root")?;
    /// # let tree = f.tree("Events")?;
    /// let pt = f.jagged::<f32>(&tree, "Muon_pt")?;
    /// for event_pts in pt.iter() {
    ///     println!("{} muons", event_pts.len());
    /// }
    /// # Ok::<(), root_io::Error>(())
    /// ```
    pub fn jagged<T: Element>(&mut self, tree: &Tree, branch: &str) -> Result<Jagged<T>> {
        let raw = self.column_of::<T>(tree, branch)?;
        let mut values = Vec::with_capacity(raw.data.len() / T::SIZE);
        let mut offsets = Vec::with_capacity(raw.len() + 1);
        offsets.push(0);
        for e in 0..raw.len() {
            values.extend(raw.entry(e).chunks_exact(T::SIZE).map(T::decode));
            offsets.push(values.len());
        }
        Ok(Jagged::from_parts(values, offsets))
    }

    /// Compression algorithm recorded in the file header.
    pub fn compression(&self) -> i32 {
        self.header.compress
    }
}
