//! Writing ROOT files.
//!
//! The layout produced mirrors what ROOT itself writes:
//!
//! ```text
//! 0                the 100-byte file header
//! 100              the file's own key, whose payload carries the directory record
//! ...              object records, each introduced by its own key
//! fSeekKeys        the directory's key list
//! fSeekFree        the free-segment list
//! fEND             end of file
//! ```

pub mod tree;

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::compress;
use crate::wbuffer::WBuffer;
use crate::error::{Context, Error, Result};
use tree::{serialize_tree, TreeWriter};

/// ROOT version stamp written into the header.
const FILE_VERSION: i32 = 63800;
/// zlib, level 1 — ROOT's default compression setting.
const COMPRESS_SETTING: i32 = 101;
const ZLIB_LEVEL: u8 = 1;
/// Records at or below this size are stored uncompressed, as ROOT does.
const COMPRESS_THRESHOLD: usize = 256;
const KEY_VERSION: i16 = 4;
const FILE_BEGIN: u64 = 100;

/// A key that has been placed in the file.
struct PlacedKey {
    class: String,
    name: String,
    title: String,
    seek: u64,
    nbytes: i32,
    obj_len: i32,
    cycle: i16,
}

/// Encoded length of a `TString`: one length byte, or the escape marker plus a
/// 32-bit length once the string reaches 255 bytes.
fn tstring_len(s: &str) -> usize {
    if s.len() < 255 {
        1 + s.len()
    } else {
        5 + s.len()
    }
}

fn push_tstring(v: &mut Vec<u8>, s: &str) {
    if s.len() < 255 {
        v.push(s.len() as u8);
    } else {
        v.push(255);
        v.extend_from_slice(&(s.len() as u32).to_be_bytes());
    }
    v.extend_from_slice(s.as_bytes());
}

/// Length of a key header for the given strings, using 32-bit seeks.
fn key_len(class: &str, name: &str, title: &str) -> i16 {
    (26 + tstring_len(class) + tstring_len(name) + tstring_len(title)) as i16
}

/// ROOT's packed date/time: year since 1995, month, day, hour, minute, second.
fn root_datime() -> u32 {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0) as i64;
    let days = secs.div_euclid(86_400);
    let tod = secs.rem_euclid(86_400);
    let (hour, min, sec) = (tod / 3600, (tod % 3600) / 60, tod % 60);

    // Civil date from days since the Unix epoch.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };

    let yy = (year - 1995).max(0) as u32;
    (yy << 26) | ((m as u32) << 22) | ((d as u32) << 17) | ((hour as u32) << 12)
        | ((min as u32) << 6)
        | sec as u32
}

/// The header in front of every record in the file.
struct KeyHeader<'a> {
    class: &'a str,
    name: &'a str,
    title: &'a str,
    /// Where this key sits in the file.
    seek: u64,
    /// The directory owning it; zero for the file's own key.
    seek_pdir: u64,
    /// Payload size once decompressed.
    obj_len: i32,
    /// Total size on disk, this header included.
    nbytes: i32,
    cycle: i16,
    datime: u32,
}

impl KeyHeader<'_> {
    fn len(&self) -> i16 {
        key_len(self.class, self.name, self.title)
    }

    /// Serialises the header, using 32-bit seeks.
    fn encode(&self) -> Vec<u8> {
        let klen = self.len();
        let mut v = Vec::with_capacity(klen as usize);
        v.extend_from_slice(&self.nbytes.to_be_bytes());
        v.extend_from_slice(&KEY_VERSION.to_be_bytes());
        v.extend_from_slice(&self.obj_len.to_be_bytes());
        v.extend_from_slice(&self.datime.to_be_bytes());
        v.extend_from_slice(&klen.to_be_bytes());
        v.extend_from_slice(&self.cycle.to_be_bytes());
        v.extend_from_slice(&(self.seek as i32).to_be_bytes());
        v.extend_from_slice(&(self.seek_pdir as i32).to_be_bytes());
        for s in [self.class, self.name, self.title] {
            push_tstring(&mut v, s);
        }
        v
    }
}

/// Builds a ROOT file in memory, then writes it out.
///
/// ```no_run
/// use root_io::write::{tree::TreeWriter, RootWriter};
///
/// let mut out = RootWriter::create("candidates.root");
/// out.write_named("provenance", "produced by allegro")?;
///
/// let mut tree = TreeWriter::new("Candidates", "one row per candidate");
/// tree.column("mass", &[125.1f32, 91.2]);
/// out.write_tree(tree)?;
///
/// out.finish()?;
/// # Ok::<(), root_io::Error>(())
/// ```
pub struct RootWriter {
    data: Vec<u8>,
    /// The file's own name, which ROOT stores inside it.
    name: String,
    path: PathBuf,
    keys: Vec<PlacedKey>,
    datime: u32,
    uuid: [u8; 16],
}

impl RootWriter {
    /// Prepares a file at `path`. The name ROOT records inside it is the last
    /// component of the path, as `TFile` does.
    pub fn create(path: impl AsRef<Path>) -> Self {
        let path = path.as_ref().to_path_buf();
        let name = path
            .file_name()
            .map_or_else(|| path.to_string_lossy(), |n| n.to_string_lossy())
            .into_owned();

        let mut uuid = [0u8; 16];
        rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut uuid);
        // RFC 4122 version 4 markers.
        uuid[6] = (uuid[6] & 0x0f) | 0x40;
        uuid[8] = (uuid[8] & 0x3f) | 0x80;

        let mut w = Self {
            data: Vec::new(),
            name,
            path,
            keys: Vec::new(),
            datime: root_datime(),
            uuid,
        };
        // The header and the file's own key are patched in at the end; leave
        // room for both.
        let reserved = FILE_BEGIN as usize + w.file_key_span();
        w.data.resize(reserved, 0);
        w
    }

    /// The path this writer will produce.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// A key for a record of this file, with the fields every record shares.
    fn key<'a>(
        &'a self,
        class: &'a str,
        name: &'a str,
        title: &'a str,
        seek: u64,
        obj_len: i32,
        nbytes: i32,
    ) -> KeyHeader<'a> {
        KeyHeader {
            class,
            name,
            title,
            seek,
            seek_pdir: FILE_BEGIN,
            obj_len,
            nbytes,
            cycle: 1,
            datime: self.datime,
        }
    }

    /// Bytes occupied by the file's own key and its directory payload.
    fn file_key_span(&self) -> usize {
        let klen = key_len("TFile", &self.name, "") as usize;
        klen + self.dir_payload_len()
    }

    fn dir_payload_len(&self) -> usize {
        // name + title + directory record + UUID + ROOT's reserved slack.
        tstring_len(&self.name) + tstring_len("") + 30 + 18 + 12
    }

    fn nbytes_name(&self) -> i32 {
        key_len("TFile", &self.name, "") as i32
            + tstring_len(&self.name) as i32
            + tstring_len("") as i32
    }

    /// Appends a record and remembers its key.
    ///
    /// The payload is whatever the class serialises to, so a crate modelling a
    /// ROOT class can place its own objects in the file.
    pub fn write_object(&mut self, class: &str, name: &str, title: &str, payload: &[u8]) -> Result<()> {
        let obj_len = payload.len();
        let stored = if obj_len > COMPRESS_THRESHOLD {
            let packed = compress::compress_zlib(payload, ZLIB_LEVEL)?;
            if packed.len() < obj_len {
                packed
            } else {
                payload.to_vec()
            }
        } else {
            payload.to_vec()
        };

        let seek = self.data.len() as u64;
        let klen = key_len(class, name, title);
        let nbytes = klen as i32 + stored.len() as i32;
        let header = self
            .key(class, name, title, seek, obj_len as i32, nbytes)
            .encode();
        self.data.extend_from_slice(&header);
        self.data.extend_from_slice(&stored);

        self.keys.push(PlacedKey {
            class: class.to_string(),
            name: name.to_string(),
            title: title.to_string(),
            seek,
            nbytes,
            obj_len: obj_len as i32,
            cycle: 1,
        });
        Ok(())
    }

    /// Writes a `TNamed`, as ROOT does for free-standing metadata.
    pub fn write_named(&mut self, name: &str, title: &str) -> Result<()> {
        let mut w = WBuffer::new(0);
        w.tnamed(name, title, 0);
        let payload = w.into_vec();
        self.write_object("TNamed", name, title, &payload)
    }

    /// Writes each column's basket, then the tree object referring to them.
    ///
    /// Fails if the columns disagree on how many rows they hold.
    pub fn write_tree(&mut self, mut tree: TreeWriter) -> Result<()> {
        let entries = tree.rows()? as i64;
        let (name, title) = (tree.name.clone(), tree.title.clone());

        for b in &mut tree.branches {
            let data_len = b.payload.len();
            let basket_key_len = key_len("TBasket", &b.name, &name) as usize + 19;
            let last = (basket_key_len + data_len) as i32;

            let stored = if data_len > COMPRESS_THRESHOLD {
                let packed = compress::compress_zlib(&b.payload, ZLIB_LEVEL)?;
                if packed.len() < data_len {
                    packed
                } else {
                    b.payload.clone()
                }
            } else {
                b.payload.clone()
            };

            let seek = self.data.len() as u64;
            let nbytes = (basket_key_len + stored.len()) as i32;
            let mut header = KeyHeader {
                class: "TBasket",
                name: &b.name,
                title: &name,
                seek,
                seek_pdir: FILE_BEGIN,
                obj_len: data_len as i32,
                nbytes,
                cycle: 1,
                datime: self.datime,
            }
            .encode();
            // The key length of a basket includes the basket header that
            // follows it, so patch the value encode_key computed.
            header[14..16].copy_from_slice(&(basket_key_len as i16).to_be_bytes());

            // TBasket header.
            header.extend_from_slice(&3i16.to_be_bytes()); // version
            header.extend_from_slice(&tree::basket_buffer_size(data_len).to_be_bytes());
            header.extend_from_slice(&(b.entry_size() as i32).to_be_bytes()); // fNevBufSize
            header.extend_from_slice(&(entries as i32).to_be_bytes()); // fNevBuf
            header.extend_from_slice(&last.to_be_bytes()); // fLast
            header.push(0); // not header-only

            debug_assert_eq!(header.len(), basket_key_len);
            self.data.extend_from_slice(&header);
            self.data.extend_from_slice(&stored);

            b.basket_seek = seek as i64;
            b.basket_bytes = nbytes;
            b.tot_bytes = (basket_key_len + data_len) as i64;
            b.zip_bytes = nbytes as i64;
        }

        let klen = key_len("TTree", &name, &title) as usize;
        let payload = serialize_tree(&name, &title, entries, &tree.branches, klen);
        self.write_object("TTree", &name, &title, &payload)
    }

    /// Finishes the file: key list, free list, directory record and header.
    pub fn finish(mut self) -> Result<()> {
        // --- key list ----------------------------------------------------
        let seek_keys = self.data.len() as u64;
        let mut list = Vec::new();
        list.extend_from_slice(&(self.keys.len() as i32).to_be_bytes());
        for k in &self.keys {
            let mut header = self.key(&k.class, &k.name, &k.title, k.seek, k.obj_len, k.nbytes);
            header.cycle = k.cycle;
            list.extend_from_slice(&header.encode());
        }
        let list_klen = key_len("TFile", &self.name, "");
        let nbytes_keys = list_klen as i32 + list.len() as i32;
        let header = self
            .key(
                "TFile",
                &self.name,
                "",
                seek_keys,
                list.len() as i32,
                nbytes_keys,
            )
            .encode();
        self.data.extend_from_slice(&header);
        self.data.extend_from_slice(&list);

        // --- free segment list -------------------------------------------
        let seek_free = self.data.len() as u64;
        let free_klen = key_len("TFile", &self.name, "");
        let free_payload_len = 10i32; // version + fFirst + fLast, 32-bit seeks
        let nbytes_free = free_klen as i32 + free_payload_len;
        let file_end = seek_free + nbytes_free as u64;
        let fheader = self
            .key(
                "TFile",
                &self.name,
                "",
                seek_free,
                free_payload_len,
                nbytes_free,
            )
            .encode();
        self.data.extend_from_slice(&fheader);
        self.data.extend_from_slice(&1i16.to_be_bytes()); // TFree version
        self.data
            .extend_from_slice(&(file_end as i32).to_be_bytes()); // fFirst
        self.data
            .extend_from_slice(&2_000_000_000i32.to_be_bytes()); // fLast

        // --- the file's own key and directory record ----------------------
        let klen = key_len("TFile", &self.name, "") as usize;
        let dir_len = self.dir_payload_len();
        let mut key = KeyHeader {
            class: "TFile",
            name: &self.name,
            title: "",
            seek: FILE_BEGIN,
            // The file's own key has no parent directory.
            seek_pdir: 0,
            obj_len: dir_len as i32,
            nbytes: (klen + dir_len) as i32,
            cycle: 1,
            datime: self.datime,
        }
        .encode();
        push_tstring(&mut key, &self.name);
        push_tstring(&mut key, ""); // empty title
        key.extend_from_slice(&5i16.to_be_bytes()); // directory version
        key.extend_from_slice(&self.datime.to_be_bytes()); // fDatimeC
        key.extend_from_slice(&self.datime.to_be_bytes()); // fDatimeM
        key.extend_from_slice(&nbytes_keys.to_be_bytes());
        key.extend_from_slice(&self.nbytes_name().to_be_bytes());
        key.extend_from_slice(&(FILE_BEGIN as i32).to_be_bytes()); // fSeekDir
        key.extend_from_slice(&0i32.to_be_bytes()); // fSeekParent
        key.extend_from_slice(&(seek_keys as i32).to_be_bytes());
        key.extend_from_slice(&1u16.to_be_bytes()); // TUUID version
        key.extend_from_slice(&self.uuid);
        key.resize(klen + dir_len, 0);
        self.data[FILE_BEGIN as usize..FILE_BEGIN as usize + key.len()].copy_from_slice(&key);

        // --- header --------------------------------------------------------
        let mut h = Vec::with_capacity(100);
        h.extend_from_slice(b"root");
        h.extend_from_slice(&FILE_VERSION.to_be_bytes());
        h.extend_from_slice(&(FILE_BEGIN as i32).to_be_bytes());
        h.extend_from_slice(&(file_end as i32).to_be_bytes()); // fEND
        h.extend_from_slice(&(seek_free as i32).to_be_bytes());
        h.extend_from_slice(&nbytes_free.to_be_bytes());
        h.extend_from_slice(&1i32.to_be_bytes()); // nfree
        h.extend_from_slice(&self.nbytes_name().to_be_bytes());
        h.push(4); // fUnits: 32-bit seeks
        h.extend_from_slice(&COMPRESS_SETTING.to_be_bytes());
        h.extend_from_slice(&0i32.to_be_bytes()); // fSeekInfo: no streamer record
        h.extend_from_slice(&0i32.to_be_bytes()); // fNbytesInfo
        h.extend_from_slice(&1u16.to_be_bytes()); // TUUID version
        h.extend_from_slice(&self.uuid);
        h.resize(100, 0);
        self.data[..100].copy_from_slice(&h);

        if self.data.len() as u64 != file_end {
            return Err(Error::format(format!(
                "internal inconsistency: buffer is {} bytes but fEND is {file_end}",
                self.data.len()
            )));
        }

        std::fs::write(&self.path, &self.data)
            .context(format!("writing {}", self.path.display()))
    }
}
