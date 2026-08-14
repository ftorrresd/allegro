//! Framed request/response transport for a single XRootD server connection.

use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use crate::proto::*;
use crate::error::{Context, Error, Result};

/// The byte stream underneath the XRootD framing.
///
/// XRootD 5 servers may demand that the connection switch to TLS part-way
/// through the session (typically right before `kXR_login`), so the transport
/// has to be replaceable in place.
pub enum Transport {
    Plain(TcpStream),
    Tls(Box<crate::tls::TlsStream>),
    /// Transient state while the socket is being moved into the TLS layer.
    Moving,
}

impl Read for Transport {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Transport::Plain(s) => s.read(buf),
            Transport::Tls(s) => s.read(buf),
            Transport::Moving => Err(io::Error::other("transport unavailable")),
        }
    }
}

impl Write for Transport {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Transport::Plain(s) => s.write(buf),
            Transport::Tls(s) => s.write(buf),
            Transport::Moving => Err(io::Error::other("transport unavailable")),
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        match self {
            Transport::Plain(s) => s.flush(),
            Transport::Tls(s) => s.flush(),
            Transport::Moving => Err(io::Error::other("transport unavailable")),
        }
    }
}

/// A raw server response: header plus the body that followed it.
pub struct Response {
    pub header: ResponseHeader,
    pub body: Vec<u8>,
}

impl Response {
    /// Turns a `kXR_error` body into a structured error.
    pub fn as_server_error(&self) -> Error {
        let Some(code) = be_i32(&self.body) else {
            return Error::protocol("kXR_error response with no detail");
        };
        Error::Server {
            code,
            message: cstr(&self.body[4..]),
        }
    }

    /// The error for a status the caller does not handle.
    ///
    /// A `kXR_error` keeps the server's own code and message; anything else
    /// becomes a protocol error naming the request it answered.
    pub fn unexpected(&self, request: &str) -> Error {
        match self.header.status {
            Status::Error => self.as_server_error(),
            other => Error::protocol(format!("unexpected {other} response to {request}")),
        }
    }

    /// The response, unless the server answered anything but `kXR_ok`.
    pub fn expect_ok(&self, request: &str) -> Result<&Self> {
        match self.header.status {
            Status::Ok => Ok(self),
            _ => Err(self.unexpected(request)),
        }
    }
}

pub struct Connection {
    pub transport: Transport,
    pub host: String,
    pub port: u16,
    next_streamid: u16,
    /// Flags returned by the server's `kXR_protocol` response.
    pub server_flags: u32,
    /// Security requirements string, e.g. `&P=gsi,v:10600,c:ssl,ca:...&P=ztn`.
    pub sec_token: Option<String>,
    pub protocol_version: i32,
}

impl Connection {
    pub fn connect(host: &str, port: u16, timeout: Duration) -> Result<Self> {
        // Try every resolved address so IPv4-only and IPv6-only hosts both work.
        let addrs = std::net::ToSocketAddrs::to_socket_addrs(&(host, port))
            .context(format!("cannot resolve {host}:{port}"))?;
        let mut last_err = None;
        let stream = addrs
            .filter_map(|addr| {
                TcpStream::connect_timeout(&addr, timeout)
                    .map_err(|e| last_err = Some(e))
                    .ok()
            })
            .next()
            .ok_or_else(|| {
                let cause = last_err
                    .unwrap_or_else(|| io::Error::other("host resolved to no addresses"));
                Error::io(format!("cannot connect to {host}:{port}"), cause)
            })?;
        stream.set_nodelay(true).context("set TCP_NODELAY")?;
        stream
            .set_read_timeout(Some(Duration::from_secs(300)))
            .context("set read timeout")?;
        stream
            .set_write_timeout(Some(Duration::from_secs(120)))
            .context("set write timeout")?;

        Ok(Self {
            transport: Transport::Plain(stream),
            host: host.to_string(),
            port,
            next_streamid: 1,
            server_flags: 0,
            sec_token: None,
            protocol_version: 0,
        })
    }

    pub fn next_streamid(&mut self) -> u16 {
        let id = self.next_streamid;
        self.next_streamid = self.next_streamid.wrapping_add(1).max(1);
        id
    }

    /// Reads one response frame (header + body).
    pub fn read_response(&mut self) -> Result<Response> {
        let mut hdr = [0u8; RESP_HEADER_LEN];
        self.transport
            .read_exact(&mut hdr)
            .context(format!("reading response header from {}", self.host))?;
        let header = ResponseHeader::parse(&hdr);
        if header.dlen < 0 {
            return Err(Error::protocol(format!(
                "negative payload length {} in response",
                header.dlen
            )));
        }
        let mut body = vec![0u8; header.dlen as usize];
        self.transport
            .read_exact(&mut body)
            .context(format!("reading {} response bytes", header.dlen))?;
        Ok(Response { header, body })
    }

    /// Performs the 20-byte initial handshake and returns (protover, msgval).
    pub fn handshake(&mut self) -> Result<(i32, i32)> {
        self.transport
            .write_all(&initial_handshake())
            .context("sending initial handshake")?;
        self.transport.flush().context("flushing handshake")?;
        let resp = self.read_response()?;
        let Some((protover, msgval)) = be_i32(&resp.body)
            .zip(resp.body.get(4..).and_then(be_i32))
        else {
            return Err(Error::protocol("short handshake response"));
        };
        self.protocol_version = protover;
        Ok((protover, msgval))
    }

    /// Sends `kXR_protocol`, recording the server's flags and security token.
    pub fn protocol(&mut self, request_tls: bool) -> Result<()> {
        let sid = self.next_streamid();
        let mut req = Request::new(sid, KXR_PROTOCOL);
        req.param_i32(0, CLIENT_PROTOCOL_VERSION);
        let mut flags = KXR_SECREQS | KXR_ABLE_TLS;
        if request_tls {
            flags |= KXR_WANT_TLS;
        }
        // clientpv occupies bytes 0..4 of the parameter area; flags is byte 4
        // and `expect` byte 5. Declaring kXR_ExpLogin matters: the server
        // reports its TLS requirements based on what we say we will do next.
        req.param_u8(4, flags);
        req.param_u8(5, KXR_EXP_LOGIN);
        let msg = req.finish(&[]);
        self.send_request(&msg)?;

        let resp = self.read_response()?;
        resp.expect_ok("kXR_protocol")?;

        let Some((version, flags)) = be_i32(&resp.body)
            .zip(resp.body.get(4..).and_then(be_u32))
        else {
            return Err(Error::protocol("short kXR_protocol response"));
        };
        self.protocol_version = version;
        self.server_flags = flags;

        // The security requirements, when present, start at the first '&'.
        self.sec_token = resp
            .body
            .get(8..)
            .and_then(|rest| rest.iter().position(|&b| b == b'&').map(|at| &rest[at..]))
            .map(cstr);
        Ok(())
    }

    pub fn requires_tls_for_login(&self) -> bool {
        self.server_flags & (KXR_GOTO_TLS | KXR_TLS_LOGIN) != 0
    }

    pub fn requires_tls_for_data(&self) -> bool {
        self.server_flags & (KXR_TLS_DATA | KXR_TLS_SESS | KXR_GOTO_TLS) != 0
    }

    /// Switches the live connection to TLS, as required before `kXR_login`
    /// on servers that advertise `kXR_tlsLogin`.
    pub fn upgrade_to_tls(&mut self) -> Result<()> {
        let host = self.host.clone();
        match std::mem::replace(&mut self.transport, Transport::Moving) {
            Transport::Plain(sock) => {
                let tls = crate::tls::TlsStream::upgrade(sock, &host)?;
                self.transport = Transport::Tls(Box::new(tls));
                Ok(())
            }
            other => {
                self.transport = other;
                Err(Error::tls("connection is already using TLS"))
            }
        }
    }

    pub fn is_tls(&self) -> bool {
        matches!(self.transport, Transport::Tls(_))
    }

    /// The peer certificate chain from the TLS handshake, if any.
    pub fn peer_certificates(&self) -> Option<Vec<Vec<u8>>> {
        match &self.transport {
            Transport::Tls(s) => s.peer_certificates(),
            _ => None,
        }
    }

    pub fn send_request(&mut self, msg: &[u8]) -> Result<()> {
        self.transport
            .write_all(msg)
            .context(format!("sending request to {}", self.host))?;
        self.transport
            .flush()
            .context(format!("flushing request to {}", self.host))
    }
}
