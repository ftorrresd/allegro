//! The `XrdSut` buffer format that carries GSI handshake data.
//!
//! Wire layout, all integers big-endian:
//!
//! ```text
//! "gsi\0"                     protocol name, NUL terminated
//! int32 step                  kXGC_*/kXGS_* handshake step
//! { int32 type, int32 size, size bytes }...
//! int32 0                     kXRS_none terminator
//! ```

/// Bucket type codes (`kXRSBucketTypes`).
pub const KXRS_NONE: i32 = 0;
pub const KXRS_CRYPTOMOD: i32 = 3000;
pub const KXRS_MAIN: i32 = 3001;
pub const KXRS_PUK: i32 = 3004;
pub const KXRS_CIPHER: i32 = 3005;
pub const KXRS_RTAG: i32 = 3006;
pub const KXRS_SIGNED_RTAG: i32 = 3007;
pub const KXRS_USER: i32 = 3008;
pub const KXRS_MESSAGE: i32 = 3011;
pub const KXRS_VERSION: i32 = 3014;
pub const KXRS_STATUS: i32 = 3015;
pub const KXRS_CLNT_OPTS: i32 = 3019;
pub const KXRS_ERROR_CODE: i32 = 3020;
pub const KXRS_X509: i32 = 3022;
pub const KXRS_ISSUER_HASH: i32 = 3023;
pub const KXRS_CIPHER_ALG: i32 = 3025;
pub const KXRS_MD_ALG: i32 = 3026;

/// Client handshake steps (`kgsiClientSteps`).
pub const KXGC_CERTREQ: i32 = 1000;
pub const KXGC_CERT: i32 = 1001;
pub const KXGC_SIGPXY: i32 = 1002;

/// Server handshake steps (`kgsiServerSteps`).
pub const KXGS_INIT: i32 = 2000;
pub const KXGS_CERT: i32 = 2001;
pub const KXGS_PXYREQ: i32 = 2002;

pub fn bucket_name(t: i32) -> &'static str {
    match t {
        KXRS_CRYPTOMOD => "cryptomod",
        KXRS_MAIN => "main",
        KXRS_PUK => "puk",
        KXRS_CIPHER => "cipher",
        KXRS_RTAG => "rtag",
        KXRS_SIGNED_RTAG => "signed_rtag",
        KXRS_USER => "user",
        KXRS_MESSAGE => "message",
        KXRS_VERSION => "version",
        KXRS_STATUS => "status",
        KXRS_CLNT_OPTS => "clnt_opts",
        KXRS_ERROR_CODE => "error_code",
        KXRS_X509 => "x509",
        KXRS_ISSUER_HASH => "issuer_hash",
        KXRS_CIPHER_ALG => "cipher_alg",
        KXRS_MD_ALG => "md_alg",
        _ => "unknown",
    }
}

#[derive(Clone)]
pub struct Bucket {
    pub kind: i32,
    pub data: Vec<u8>,
}

/// An ordered list of buckets plus the handshake step.
#[derive(Clone, Default)]
pub struct SutBuffer {
    pub protocol: String,
    pub step: i32,
    pub buckets: Vec<Bucket>,
}

impl SutBuffer {
    pub fn new(step: i32) -> Self {
        Self {
            protocol: "gsi".into(),
            step,
            buckets: Vec::new(),
        }
    }

    pub fn get(&self, kind: i32) -> Option<&[u8]> {
        self.buckets
            .iter()
            .find(|b| b.kind == kind)
            .map(|b| b.data.as_slice())
    }

    pub fn get_string(&self, kind: i32) -> Option<String> {
        self.get(kind).map(|d| {
            let end = d.iter().position(|&b| b == 0).unwrap_or(d.len());
            String::from_utf8_lossy(&d[..end]).into_owned()
        })
    }

    pub fn get_i32(&self, kind: i32) -> Option<i32> {
        self.get(kind).and_then(|d| {
            (d.len() >= 4).then(|| i32::from_be_bytes([d[0], d[1], d[2], d[3]]))
        })
    }

    pub fn add(&mut self, kind: i32, data: Vec<u8>) {
        self.buckets.push(Bucket { kind, data });
    }

    pub fn add_str(&mut self, kind: i32, s: &str) {
        // Strings travel without a trailing NUL, as `XrdSutBucket(XrdOucString&)`
        // stores exactly the string's bytes.
        self.add(kind, s.as_bytes().to_vec());
    }

    pub fn add_i32(&mut self, kind: i32, v: i32) {
        self.add(kind, v.to_be_bytes().to_vec());
    }

    /// Replaces a bucket's contents, appending it if not present.
    pub fn set(&mut self, kind: i32, data: Vec<u8>) {
        match self.buckets.iter_mut().find(|b| b.kind == kind) {
            Some(b) => b.data = data,
            None => self.add(kind, data),
        }
    }

    pub fn set_str(&mut self, kind: i32, s: &str) {
        self.set(kind, s.as_bytes().to_vec());
    }

    /// Drops a bucket, the effect of `XrdSutBuffer::Deactivate`.
    pub fn remove(&mut self, kind: i32) {
        self.buckets.retain(|b| b.kind != kind);
    }

    pub fn serialize(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(self.protocol.as_bytes());
        out.push(0);
        out.extend_from_slice(&self.step.to_be_bytes());
        for b in &self.buckets {
            out.extend_from_slice(&b.kind.to_be_bytes());
            out.extend_from_slice(&(b.data.len() as i32).to_be_bytes());
            out.extend_from_slice(&b.data);
        }
        out.extend_from_slice(&KXRS_NONE.to_be_bytes());
        out
    }

    pub fn parse(buf: &[u8]) -> Result<Self, String> {
        let nul = buf
            .iter()
            .position(|&b| b == 0)
            .ok_or("no NUL-terminated protocol name in GSI buffer")?;
        let protocol = String::from_utf8_lossy(&buf[..nul]).into_owned();
        let mut pos = nul + 1;
        let read_i32 = |p: usize| -> Result<i32, String> {
            if p + 4 > buf.len() {
                return Err("truncated GSI buffer".into());
            }
            Ok(i32::from_be_bytes([buf[p], buf[p + 1], buf[p + 2], buf[p + 3]]))
        };
        let step = read_i32(pos)?;
        pos += 4;

        let mut buckets = Vec::new();
        loop {
            let kind = read_i32(pos)?;
            pos += 4;
            if kind == KXRS_NONE {
                break;
            }
            let size = read_i32(pos)?;
            pos += 4;
            if size < 0 || pos + size as usize > buf.len() {
                return Err(format!(
                    "bucket {} declares {} bytes but only {} remain",
                    bucket_name(kind),
                    size,
                    buf.len().saturating_sub(pos)
                ));
            }
            buckets.push(Bucket {
                kind,
                data: buf[pos..pos + size as usize].to_vec(),
            });
            pos += size as usize;
        }
        Ok(Self {
            protocol,
            step,
            buckets,
        })
    }

    /// A one-line summary for troubleshooting handshakes.
    pub fn describe(&self) -> String {
        let items: Vec<String> = self
            .buckets
            .iter()
            .map(|b| format!("{}({})", bucket_name(b.kind), b.data.len()))
            .collect();
        format!("step={} [{}]", self.step, items.join(", "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_the_wire_format() {
        let mut b = SutBuffer::new(KXGC_CERTREQ);
        b.add_str(KXRS_CRYPTOMOD, "ssl");
        b.add_i32(KXRS_VERSION, 10600);
        b.add_str(KXRS_ISSUER_HASH, "5168735f|4339b4bc");
        b.add(KXRS_MAIN, vec![1, 2, 3, 4]);

        let wire = b.serialize();
        let back = SutBuffer::parse(&wire).unwrap();
        assert_eq!(back.protocol, "gsi");
        assert_eq!(back.step, KXGC_CERTREQ);
        assert_eq!(back.get_string(KXRS_CRYPTOMOD).unwrap(), "ssl");
        assert_eq!(back.get_i32(KXRS_VERSION).unwrap(), 10600);
        assert_eq!(back.get(KXRS_MAIN).unwrap(), &[1, 2, 3, 4]);
    }

    #[test]
    fn rejects_a_bucket_that_overruns_the_buffer() {
        let mut wire = b"gsi\0".to_vec();
        wire.extend_from_slice(&KXGC_CERTREQ.to_be_bytes());
        wire.extend_from_slice(&KXRS_MAIN.to_be_bytes());
        wire.extend_from_slice(&9999i32.to_be_bytes());
        assert!(SutBuffer::parse(&wire).is_err());
    }
}
