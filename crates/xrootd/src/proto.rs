//! Wire-level constants and encoding helpers for the XRootD protocol.
//!
//! Field layouts follow XProtocol.hh from XRootD 5.9. Every integer on the
//! wire is big-endian and every client request header is exactly 24 bytes.

use std::fmt;

pub const REQ_HEADER_LEN: usize = 24;
pub const RESP_HEADER_LEN: usize = 8;

// Request codes (XReqCode).
pub const KXR_AUTH: u16 = 3000;
pub const KXR_CLOSE: u16 = 3003;
pub const KXR_PROTOCOL: u16 = 3006;
pub const KXR_LOGIN: u16 = 3007;
pub const KXR_OPEN: u16 = 3010;
pub const KXR_PING: u16 = 3011;
pub const KXR_READ: u16 = 3013;
pub const KXR_STAT: u16 = 3017;
pub const KXR_ENDSESS: u16 = 3023;

/// What a server said about a request (`XResponseType`).
///
/// Unrecognised codes are kept rather than collapsed, so a server speaking a
/// newer protocol produces a diagnosable error instead of a silent mismatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// The request succeeded and this is the whole answer.
    Ok,
    /// Part of the answer; more responses follow.
    OkSoFar,
    Attn,
    /// The authentication handshake needs another round.
    AuthMore,
    /// A `kXR_error` body, carrying a code and a message.
    Error,
    /// The file lives on another server.
    Redirect,
    Wait,
    WaitResp,
    Status,
    Unknown(u16),
}

impl From<u16> for Status {
    fn from(raw: u16) -> Self {
        match raw {
            0 => Status::Ok,
            4000 => Status::OkSoFar,
            4001 => Status::Attn,
            4002 => Status::AuthMore,
            4003 => Status::Error,
            4004 => Status::Redirect,
            4005 => Status::Wait,
            4006 => Status::WaitResp,
            4007 => Status::Status,
            other => Status::Unknown(other),
        }
    }
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Status::Ok => f.write_str("kXR_ok"),
            Status::OkSoFar => f.write_str("kXR_oksofar"),
            Status::Attn => f.write_str("kXR_attn"),
            Status::AuthMore => f.write_str("kXR_authmore"),
            Status::Error => f.write_str("kXR_error"),
            Status::Redirect => f.write_str("kXR_redirect"),
            Status::Wait => f.write_str("kXR_wait"),
            Status::WaitResp => f.write_str("kXR_waitresp"),
            Status::Status => f.write_str("kXR_status"),
            Status::Unknown(code) => write!(f, "unknown response {code}"),
        }
    }
}

// kXR_protocol request flags (RequestFlags).
pub const KXR_SECREQS: u8 = 0x01;
pub const KXR_ABLE_TLS: u8 = 0x02;
pub const KXR_WANT_TLS: u8 = 0x04;

// kXR_protocol `expect` values (ExpectFlags): what the client will do next.
pub const KXR_EXP_NONE: u8 = 0x00;
pub const KXR_EXP_LOGIN: u8 = 0x03;

// kXR_protocol response flags: TLS requirements.
pub const KXR_HAVE_TLS: u32 = 0x8000_0000;
pub const KXR_GOTO_TLS: u32 = 0x4000_0000;
pub const KXR_TLS_DATA: u32 = 0x0100_0000;
pub const KXR_TLS_GPF: u32 = 0x0200_0000;
pub const KXR_TLS_LOGIN: u32 = 0x0400_0000;
pub const KXR_TLS_SESS: u32 = 0x0800_0000;
pub const KXR_TLS_TPC: u32 = 0x1000_0000;
pub const KXR_TLS_GPFA: u32 = 0x2000_0000;

// kXR_protocol response flags: server role.
pub const KXR_IS_SERVER: u32 = 0x0000_0001;
pub const KXR_IS_MANAGER: u32 = 0x0000_0002;
pub const KXR_ATTR_META: u32 = 0x0000_0100;

// kXR_open options (XOpenRequestOption).
pub const KXR_OPEN_READ: u16 = 0x0010;
pub const KXR_RETSTAT: u16 = 0x0400;

// kXR_login ability flags (XLoginAbility) and capability version.
pub const KXR_FULLURL: u8 = 1;
pub const KXR_READRDOK: u8 = 4;
pub const KXR_HASIPV64: u8 = 8;
pub const KXR_REDIRFLAGS: u8 = 128;
/// kXR_ver005: the 2019 TLS-capable client.
pub const KXR_VER005: u8 = 5;
pub const KXR_ASYNCAP: u8 = 128;

/// The protocol version this client claims to speak (5.2.0).
pub const CLIENT_PROTOCOL_VERSION: i32 = 0x0000_0520;

/// Builds the 20-byte initial handshake: {0, 0, 0, 4, 2012}.
pub fn initial_handshake() -> [u8; 20] {
    let mut buf = [0u8; 20];
    buf[12..16].copy_from_slice(&4i32.to_be_bytes());
    buf[16..20].copy_from_slice(&2012i32.to_be_bytes());
    buf
}

/// A 24-byte client request header under construction.
///
/// Layout is always: streamid[2], requestid[2], 16 bytes of request-specific
/// parameters, dlen[4].
pub struct Request {
    pub buf: [u8; REQ_HEADER_LEN],
}

impl Request {
    pub fn new(streamid: u16, requestid: u16) -> Self {
        let mut buf = [0u8; REQ_HEADER_LEN];
        buf[0..2].copy_from_slice(&streamid.to_be_bytes());
        buf[2..4].copy_from_slice(&requestid.to_be_bytes());
        Self { buf }
    }

    /// Writes into the 16-byte parameter area, which starts at offset 4.
    pub fn param_u8(&mut self, offset: usize, v: u8) -> &mut Self {
        self.buf[4 + offset] = v;
        self
    }

    pub fn param_u16(&mut self, offset: usize, v: u16) -> &mut Self {
        self.buf[4 + offset..6 + offset].copy_from_slice(&v.to_be_bytes());
        self
    }

    pub fn param_i32(&mut self, offset: usize, v: i32) -> &mut Self {
        self.buf[4 + offset..8 + offset].copy_from_slice(&v.to_be_bytes());
        self
    }

    pub fn param_i64(&mut self, offset: usize, v: i64) -> &mut Self {
        self.buf[4 + offset..12 + offset].copy_from_slice(&v.to_be_bytes());
        self
    }

    pub fn param_bytes(&mut self, offset: usize, v: &[u8]) -> &mut Self {
        self.buf[4 + offset..4 + offset + v.len()].copy_from_slice(v);
        self
    }

    /// Finalises the header by setting dlen and returns header + payload.
    pub fn finish(mut self, payload: &[u8]) -> Vec<u8> {
        self.buf[20..24].copy_from_slice(&(payload.len() as i32).to_be_bytes());
        let mut out = Vec::with_capacity(REQ_HEADER_LEN + payload.len());
        out.extend_from_slice(&self.buf);
        out.extend_from_slice(payload);
        out
    }
}

/// A decoded server response header.
#[derive(Debug, Clone, Copy)]
pub struct ResponseHeader {
    pub streamid: u16,
    pub status: Status,
    pub dlen: i32,
}

impl ResponseHeader {
    pub fn parse(buf: &[u8; RESP_HEADER_LEN]) -> Self {
        Self {
            streamid: u16::from_be_bytes([buf[0], buf[1]]),
            status: u16::from_be_bytes([buf[2], buf[3]]).into(),
            dlen: i32::from_be_bytes([buf[4], buf[5], buf[6], buf[7]]),
        }
    }
}

/// Reads a big-endian `i32` from the front of a buffer, if it is long enough.
pub fn be_i32(buf: &[u8]) -> Option<i32> {
    buf.first_chunk().copied().map(i32::from_be_bytes)
}

/// Reads a big-endian `u32` from the front of a buffer, if it is long enough.
pub fn be_u32(buf: &[u8]) -> Option<u32> {
    buf.first_chunk().copied().map(u32::from_be_bytes)
}

/// The text up to the first NUL, which is how XRootD returns host names,
/// error messages and security tokens.
pub fn cstr(buf: &[u8]) -> String {
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..end]).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses_round_trip_and_name_themselves() {
        assert_eq!(Status::from(0), Status::Ok);
        assert_eq!(Status::from(4003), Status::Error);
        assert_eq!(Status::from(9999), Status::Unknown(9999));
        assert_eq!(Status::Redirect.to_string(), "kXR_redirect");
        assert_eq!(Status::Unknown(7).to_string(), "unknown response 7");
    }

    #[test]
    fn reads_prefixed_scalars_and_strings() {
        assert_eq!(be_i32(&(-5i32).to_be_bytes()), Some(-5));
        assert_eq!(be_i32(&[0, 1]), None);
        assert_eq!(cstr(b"host.example\0trailing"), "host.example");
        assert_eq!(cstr(b"no nul"), "no nul");
    }
}
