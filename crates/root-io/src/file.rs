//! The ROOT file container: header, keys and directory records.

use crate::buffer::RBuffer;
use crate::compress;
use crate::error::{Error, Result};
use crate::source::ReadAt;

/// A `TKey`: the record header in front of every object in a ROOT file.
#[derive(Debug, Clone)]
pub struct TKey {
    /// Total size on disk, key header included.
    pub nbytes: i32,
    pub version: i16,
    /// Size of the object once decompressed.
    pub obj_len: i32,
    pub key_len: i16,
    pub cycle: i16,
    pub seek_key: u64,
    pub seek_pdir: u64,
    pub class_name: String,
    pub name: String,
    pub title: String,
}

impl TKey {
    pub fn read(b: &mut RBuffer) -> Result<Self> {
        let nbytes = b.i32()?;
        let version = b.i16()?;
        let obj_len = b.i32()?;
        let _datime = b.u32()?;
        let key_len = b.i16()?;
        let cycle = b.i16()?;
        // Keys in files above 2 GiB use 64-bit seeks, flagged by version > 1000.
        let (seek_key, seek_pdir) = if version > 1000 {
            (b.i64()? as u64, b.i64()? as u64)
        } else {
            (b.i32()? as u64, b.i32()? as u64)
        };
        let class_name = b.tstring()?;
        let name = b.tstring()?;
        let title = b.tstring()?;
        Ok(Self {
            nbytes,
            version,
            obj_len,
            key_len,
            cycle,
            seek_key,
            seek_pdir,
            class_name,
            name,
            title,
        })
    }

    /// True when the payload is stored compressed.
    pub fn is_compressed(&self) -> bool {
        self.obj_len != self.nbytes - self.key_len as i32
    }
}

#[derive(Debug, Clone)]
pub struct FileHeader {
    pub version: i32,
    pub begin: u64,
    pub end: u64,
    pub nbytes_name: i32,
    pub compress: i32,
    pub seek_info: u64,
    pub nbytes_info: i32,
    pub large_file: bool,
}

impl FileHeader {
    pub fn read(src: &mut dyn ReadAt) -> Result<Self> {
        let raw = src.read_at(0, 100)?;
        if &raw[..4] != b"root" {
            return Err(Error::format(
                "not a ROOT file: missing the 'root' magic at offset 0",
            ));
        }
        let mut b = RBuffer::new(&raw);
        b.skip(4)?;
        let version = b.i32()?;
        let begin = b.i32()? as u64;
        // Files past 2 GiB record 64-bit offsets and add 1000000 to fVersion.
        let large_file = version > 1_000_000;
        let (end, _seek_free) = if large_file {
            (b.i64()? as u64, b.i64()? as u64)
        } else {
            (b.i32()? as u64, b.i32()? as u64)
        };
        let _nbytes_free = b.i32()?;
        let _nfree = b.i32()?;
        let nbytes_name = b.i32()?;
        let _units = b.u8()?;
        let compress = b.i32()?;
        let seek_info = if large_file {
            b.i64()? as u64
        } else {
            b.i32()? as u64
        };
        let nbytes_info = b.i32()?;

        Ok(Self {
            version,
            begin,
            end,
            nbytes_name,
            compress,
            seek_info,
            nbytes_info,
            large_file,
        })
    }
}

/// Reads and decompresses the object a key points at.
pub fn read_object(src: &mut dyn ReadAt, key: &TKey) -> Result<Vec<u8>> {
    let raw = src.read_at(
        key.seek_key + key.key_len as u64,
        (key.nbytes - key.key_len as i32) as usize,
    )?;
    if key.is_compressed() {
        compress::decompress(&raw, key.obj_len as usize)
    } else {
        Ok(raw)
    }
}

/// Reads the list of keys held by the file's top-level directory.
pub fn read_top_level_keys(src: &mut dyn ReadAt, header: &FileHeader) -> Result<Vec<TKey>> {
    // The directory record sits immediately after the file's own key.
    let dir_start = header.begin + header.nbytes_name as u64;
    let raw = src.read_at(dir_start, 42)?;
    let mut b = RBuffer::new(&raw);
    let version = b.i16()?;
    let _datime_c = b.u32()?;
    let _datime_m = b.u32()?;
    let _nbytes_keys = b.i32()?;
    let _nbytes_name = b.i32()?;
    let (_seek_dir, _seek_parent, seek_keys) = if version > 1000 {
        (b.i64()? as u64, b.i64()? as u64, b.i64()? as u64)
    } else {
        (b.i32()? as u64, b.i32()? as u64, b.i32()? as u64)
    };
    if seek_keys == 0 {
        return Err(Error::format("directory has no key list"));
    }

    // At fSeekKeys there is a key whose payload is the key list itself.
    let head = src.read_at(seek_keys, 128.min((header.end - seek_keys) as usize))?;
    let mut hb = RBuffer::new(&head);
    let list_key = TKey::read(&mut hb)?;
    let payload = read_object(src, &list_key)?;

    let mut pb = RBuffer::new(&payload);
    let nkeys = pb.i32()?;
    if nkeys < 0 {
        return Err(Error::format(format!("negative key count {nkeys}")));
    }
    let mut keys = Vec::with_capacity(nkeys as usize);
    for _ in 0..nkeys {
        keys.push(TKey::read(&mut pb)?);
    }
    Ok(keys)
}
