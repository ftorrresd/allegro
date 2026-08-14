//! Byte sources a ROOT file can be read from.
//!
//! Reading is always by explicit `(offset, length)` so that only the records
//! actually needed are fetched — over XRootD each of these becomes a
//! `kXR_read` for that range rather than a download of the whole file.

use std::collections::{HashMap, VecDeque};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};

use crate::error::{Context, Error, Result};
use xrootd::XrdFile;

/// Random-access reads over some storage.
pub trait ReadAt {
    fn read_at(&mut self, offset: u64, len: usize) -> Result<Vec<u8>>;
    fn size(&self) -> u64;
    /// Where the data is coming from, for messages.
    fn describe(&self) -> String;
}

pub struct LocalSource {
    file: File,
    size: u64,
    path: String,
}

impl LocalSource {
    pub fn open(path: &str) -> Result<Self> {
        let file = File::open(path).context(format!("opening {path}"))?;
        let size = file.metadata().context(format!("stat of {path}"))?.len();
        Ok(Self {
            file,
            size,
            path: path.to_string(),
        })
    }
}

impl ReadAt for LocalSource {
    fn read_at(&mut self, offset: u64, len: usize) -> Result<Vec<u8>> {
        self.file
            .seek(SeekFrom::Start(offset))
            .context(format!("seeking to {offset} in {}", self.path))?;
        let mut buf = vec![0u8; len];
        self.file.read_exact(&mut buf).context(format!(
            "reading {len} bytes at {offset} from {}",
            self.path
        ))?;
        Ok(buf)
    }
    fn size(&self) -> u64 {
        self.size
    }
    fn describe(&self) -> String {
        self.path.clone()
    }
}

pub struct XrdSource {
    file: XrdFile,
    url: String,
}

impl XrdSource {
    pub fn open(url: &str) -> Result<Self> {
        let file = XrdFile::open(url)?;
        Ok(Self {
            file,
            url: url.to_string(),
        })
    }
}

impl ReadAt for XrdSource {
    fn read_at(&mut self, offset: u64, len: usize) -> Result<Vec<u8>> {
        let got = self.file.read_at(offset, len)?;
        if got.len() != len {
            return Err(Error::format(format!(
                "short read: asked for {len} bytes at {offset}, received {}",
                got.len()
            )));
        }
        Ok(got)
    }
    fn size(&self) -> u64 {
        self.file.size()
    }
    fn describe(&self) -> String {
        self.url.clone()
    }
}

/// Read-through cache over aligned blocks.
///
/// ROOT's access pattern is many small structural reads (header, key lists)
/// followed by larger basket reads. Serving them from aligned blocks keeps the
/// number of network round trips low without ever fetching the whole file.
pub struct CachedSource<S: ReadAt> {
    inner: S,
    block_size: usize,
    max_bytes: usize,
    blocks: HashMap<u64, Vec<u8>>,
    order: VecDeque<u64>,
    cached_bytes: usize,
    pub reads_issued: usize,
    pub bytes_fetched: u64,
}

impl<S: ReadAt> CachedSource<S> {
    pub fn new(inner: S, block_size: usize, max_bytes: usize) -> Self {
        Self {
            inner,
            block_size,
            max_bytes,
            blocks: HashMap::new(),
            order: VecDeque::new(),
            cached_bytes: 0,
            reads_issued: 0,
            bytes_fetched: 0,
        }
    }

    fn block(&mut self, index: u64) -> Result<&[u8]> {
        if !self.blocks.contains_key(&index) {
            let start = index * self.block_size as u64;
            let size = self.inner.size();
            let len = ((size - start) as usize).min(self.block_size);
            let data = self.inner.read_at(start, len)?;
            self.reads_issued += 1;
            self.bytes_fetched += data.len() as u64;

            while self.cached_bytes + data.len() > self.max_bytes {
                match self.order.pop_front() {
                    Some(old) => {
                        if let Some(v) = self.blocks.remove(&old) {
                            self.cached_bytes -= v.len();
                        }
                    }
                    None => break,
                }
            }
            self.cached_bytes += data.len();
            self.order.push_back(index);
            self.blocks.insert(index, data);
        }
        Ok(self.blocks.get(&index).expect("just inserted"))
    }
}

impl<S: ReadAt> ReadAt for CachedSource<S> {
    fn read_at(&mut self, offset: u64, len: usize) -> Result<Vec<u8>> {
        if len == 0 {
            return Ok(Vec::new());
        }
        let size = self.inner.size();
        if offset + len as u64 > size {
            return Err(Error::format(format!(
                "read of {len} bytes at {offset} runs past the {size}-byte file"
            )));
        }
        // Requests larger than a block bypass the cache: they are basket reads
        // that would evict everything else for no benefit.
        if len >= self.block_size {
            let data = self.inner.read_at(offset, len)?;
            self.reads_issued += 1;
            self.bytes_fetched += data.len() as u64;
            return Ok(data);
        }

        let bs = self.block_size as u64;
        let first = offset / bs;
        let last = (offset + len as u64 - 1) / bs;
        let mut out = Vec::with_capacity(len);
        for index in first..=last {
            let block = self.block(index)?;
            let block_start = index * bs;
            let from = offset.saturating_sub(block_start) as usize;
            let to = (((offset + len as u64) - block_start) as usize).min(block.len());
            out.extend_from_slice(&block[from..to]);
        }
        Ok(out)
    }

    fn size(&self) -> u64 {
        self.inner.size()
    }

    fn describe(&self) -> String {
        self.inner.describe()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake {
        data: Vec<u8>,
        reads: usize,
    }
    impl ReadAt for Fake {
        fn read_at(&mut self, offset: u64, len: usize) -> Result<Vec<u8>> {
            self.reads += 1;
            Ok(self.data[offset as usize..offset as usize + len].to_vec())
        }
        fn size(&self) -> u64 {
            self.data.len() as u64
        }
        fn describe(&self) -> String {
            "fake".into()
        }
    }

    #[test]
    fn cache_serves_repeated_reads_from_one_fetch() {
        let data: Vec<u8> = (0..4096u32).map(|i| (i % 256) as u8).collect();
        let mut src = CachedSource::new(
            Fake {
                data: data.clone(),
                reads: 0,
            },
            1024,
            1 << 20,
        );
        assert_eq!(src.read_at(10, 5).unwrap(), data[10..15]);
        assert_eq!(src.read_at(20, 5).unwrap(), data[20..25]);
        assert_eq!(src.reads_issued, 1, "both reads share one block");
    }

    #[test]
    fn reads_spanning_blocks_are_stitched() {
        let data: Vec<u8> = (0..4096u32).map(|i| (i % 256) as u8).collect();
        let mut src = CachedSource::new(
            Fake {
                data: data.clone(),
                reads: 0,
            },
            1024,
            1 << 20,
        );
        assert_eq!(src.read_at(1020, 8).unwrap(), data[1020..1028]);
    }

    #[test]
    fn reading_past_the_end_is_rejected() {
        let mut src = CachedSource::new(
            Fake {
                data: vec![0; 100],
                reads: 0,
            },
            64,
            1 << 20,
        );
        assert!(src.read_at(90, 20).is_err());
    }
}
