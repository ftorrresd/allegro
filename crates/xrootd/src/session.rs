//! An authenticated XRootD session and the ranged reads built on it.

use std::time::Duration;

use crate::conn::Connection;
use crate::gsi::proxy::{default_proxy_path, ProxyCredential};
use crate::gsi::{GsiAuthenticator, GsiParams};
use crate::proto::*;
use crate::error::{Error, Result};

/// Maximum number of redirects to follow before giving up.
const MAX_REDIRECTS: usize = 8;
/// Round trips the GSI handshake is allowed before it is called stuck.
const MAX_AUTH_ROUNDS: usize = 8;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
/// The port an XRootD URL means when it does not say.
pub const DEFAULT_PORT: u16 = 1094;

/// A parsed `root://host[:port]//path[?opaque]` URL.
#[derive(Debug, Clone)]
pub struct XrdUrl {
    pub host: String,
    pub port: u16,
    pub path: String,
    pub opaque: Option<String>,
}

impl XrdUrl {
    /// Whether a string names a file this client can reach.
    pub fn is_url(s: &str) -> bool {
        s.starts_with("root://") || s.starts_with("roots://")
    }

    pub fn parse(url: &str) -> Result<Self> {
        let rest = url
            .strip_prefix("root://")
            .or_else(|| url.strip_prefix("roots://"))
            .ok_or_else(|| Error::config(format!("not an xrootd URL: {url}")))?;
        // The path begins at the first '/' after the host; XRootD URLs
        // conventionally use a double slash to separate host from an absolute
        // path.
        let (authority, path_part) = match rest.find('/') {
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, ""),
        };
        let (host, port) = match authority.rsplit_once(':') {
            Some((h, p)) if !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()) => {
                (h.to_string(), p.parse().unwrap_or(DEFAULT_PORT))
            }
            _ => (authority.to_string(), DEFAULT_PORT),
        };
        if host.is_empty() {
            return Err(Error::config(format!("no host in URL: {url}")));
        }
        let (path, opaque) = match path_part.split_once('?') {
            Some((p, o)) => (p.to_string(), Some(o.to_string())),
            None => (path_part.to_string(), None),
        };
        // Collapse the leading double slash into the absolute path.
        let path = if let Some(stripped) = path.strip_prefix("//") {
            format!("/{stripped}")
        } else {
            path
        };
        if path.is_empty() {
            return Err(Error::config(format!("no path in URL: {url}")));
        }
        Ok(Self {
            host,
            port,
            path,
            opaque,
        })
    }
}

pub struct Session {
    conn: Connection,
}

impl Session {
    /// Connects, negotiates TLS if required, logs in and authenticates.
    pub fn establish(host: &str, port: u16) -> Result<Self> {
        let mut conn = Connection::connect(host, port, CONNECT_TIMEOUT)?;
        conn.handshake()?;
        conn.protocol(false)?;
        if conn.requires_tls_for_login() {
            conn.upgrade_to_tls()?;
        }

        let sec_token = login(&mut conn)?;
        let mut session = Self { conn };
        if let Some(token) = sec_token {
            session.authenticate(&token)?;
        }
        Ok(session)
    }

    /// Runs the GSI exchange until the server accepts us.
    fn authenticate(&mut self, sec_token: &str) -> Result<()> {
        let params = GsiParams::parse(sec_token)?;
        let path = default_proxy_path()?;
        let proxy = ProxyCredential::load(&path)?;
        let mut gsi = GsiAuthenticator::new(proxy, params);

        let mut payload = gsi.initial_credentials()?;
        for _ in 0..MAX_AUTH_ROUNDS {
            let resp = self.send_auth(&payload)?;
            match resp.header.status {
                Status::Ok => return Ok(()),
                Status::AuthMore => match gsi.next_credentials(&resp.body)? {
                    Some(next) => payload = next,
                    None => return Ok(()),
                },
                _ => return Err(resp.unexpected("kXR_auth")),
            }
        }
        Err(Error::auth("authentication did not converge"))
    }

    fn send_auth(&mut self, payload: &[u8]) -> Result<crate::conn::Response> {
        let sid = self.conn.next_streamid();
        let mut req = Request::new(sid, KXR_AUTH);
        // Parameter area: 12 reserved bytes then credtype[4].
        req.param_bytes(12, b"gsi\0");
        let msg = req.finish(payload);
        self.conn.send_request(&msg)?;
        self.conn.read_response()
    }
}

/// Sends `kXR_login`, returning the security token when authentication is
/// required.
fn login(conn: &mut Connection) -> Result<Option<String>> {
    let user = std::env::var("USER").unwrap_or_else(|_| "nobody".into());
    let mut uname = [0u8; 8];
    let ub = user.as_bytes();
    let n = ub.len().min(8);
    uname[..n].copy_from_slice(&ub[..n]);

    let sid = conn.next_streamid();
    let mut req = Request::new(sid, KXR_LOGIN);
    req.param_i32(0, std::process::id() as i32);
    req.param_bytes(4, &uname);
    req.param_u8(13, KXR_FULLURL | KXR_READRDOK | KXR_HASIPV64 | KXR_REDIRFLAGS);
    req.param_u8(14, KXR_VER005 | KXR_ASYNCAP);
    let msg = req.finish(b"");
    conn.send_request(&msg)?;

    let resp = conn.read_response()?;
    resp.expect_ok("kXR_login")?;

    // A security token, when the server wants us to authenticate, follows the
    // 16-byte session id.
    Ok(resp
        .body
        .get(16..)
        .map(cstr)
        .filter(|token| !token.is_empty()))
}

/// A file opened over XRootD, supporting ranged reads.
pub struct XrdFile {
    session: Session,
    handle: [u8; 4],
    size: u64,
    pub url: XrdUrl,
}

impl XrdFile {
    /// Opens `url`, following redirects from the redirector to the data server
    /// holding the replica.
    pub fn open(url: &str) -> Result<Self> {
        let mut target = XrdUrl::parse(url)?;
        let mut redirects = 0;

        loop {
            let mut session = Session::establish(&target.host, target.port)?;
            match try_open(&mut session, &target)? {
                OpenOutcome::Opened { handle, size } => {
                    return Ok(Self {
                        session,
                        handle,
                        size,
                        url: target,
                    })
                }
                OpenOutcome::Redirect { target: next } => {
                    redirects += 1;
                    if redirects > MAX_REDIRECTS {
                        return Err(Error::protocol(format!(
                            "too many redirects while opening {url}"
                        )));
                    }
                    target = next;
                }
            }
        }
    }

    pub fn size(&self) -> u64 {
        self.size
    }

    /// Reads `len` bytes at `offset`, issuing `kXR_read` and reassembling any
    /// partial (`kXR_oksofar`) responses.
    pub fn read_at(&mut self, offset: u64, len: usize) -> Result<Vec<u8>> {
        if len == 0 {
            return Ok(Vec::new());
        }
        let sid = self.session.conn.next_streamid();
        let mut req = Request::new(sid, KXR_READ);
        req.param_bytes(0, &self.handle);
        req.param_i64(4, offset as i64);
        req.param_i32(12, len as i32);
        let msg = req.finish(b"");
        self.session.conn.send_request(&msg)?;

        let mut out = Vec::with_capacity(len);
        loop {
            let resp = self.session.conn.read_response()?;
            match resp.header.status {
                // A large read arrives as several kXR_oksofar frames followed
                // by a kXR_ok one.
                Status::OkSoFar => out.extend_from_slice(&resp.body),
                Status::Ok => {
                    out.extend_from_slice(&resp.body);
                    return Ok(out);
                }
                _ => return Err(resp.unexpected("kXR_read")),
            }
        }
    }

    pub fn close(&mut self) -> Result<()> {
        let sid = self.session.conn.next_streamid();
        let mut req = Request::new(sid, KXR_CLOSE);
        req.param_bytes(0, &self.handle);
        let msg = req.finish(b"");
        self.session.conn.send_request(&msg)?;
        let resp = self.session.conn.read_response()?;
        match resp.header.status {
            Status::Error => Err(resp.as_server_error()),
            _ => Ok(()),
        }
    }
}

/// What a `kXR_open` came back with.
enum OpenOutcome {
    Opened { handle: [u8; 4], size: u64 },
    /// The file is elsewhere; `target` is where to ask next.
    Redirect { target: XrdUrl },
}

fn try_open(session: &mut Session, url: &XrdUrl) -> Result<OpenOutcome> {
    let mut path = url.path.clone();
    if let Some(o) = &url.opaque {
        path.push('?');
        path.push_str(o);
    }

    let sid = session.conn.next_streamid();
    let mut req = Request::new(sid, KXR_OPEN);
    // Parameter area: mode[2], options[2], reserved[12].
    req.param_u16(0, 0);
    req.param_u16(2, KXR_OPEN_READ | KXR_RETSTAT);
    let msg = req.finish(path.as_bytes());
    session.conn.send_request(&msg)?;

    let resp = session.conn.read_response()?;
    match resp.header.status {
        Status::Ok => {
            let Some(handle) = resp.body.first_chunk::<4>().copied() else {
                return Err(Error::protocol("kXR_open response has no file handle"));
            };
            // With kXR_retstat the handle is followed by cpsize[4], cptype[4]
            // and then the stat string "id size flags modtime".
            let size = resp
                .body
                .get(12..)
                .and_then(parse_stat_size)
                .unwrap_or_default();
            Ok(OpenOutcome::Opened { handle, size })
        }
        Status::Redirect => {
            let Some(port) = be_i32(&resp.body) else {
                return Err(Error::protocol("kXR_redirect response is too short"));
            };
            let port = u16::try_from(port).unwrap_or(DEFAULT_PORT);
            let target = redirect_target(&cstr(&resp.body[4..]), port, url)?;
            Ok(OpenOutcome::Redirect { target })
        }
        _ => Err(resp.unexpected("kXR_open")),
    }
}

/// Resolves where a `kXR_redirect` is pointing.
///
/// The body usually names a host, optionally with `?opaque` appended, and the
/// path stays the one we asked for. A redirector may instead hand back a whole
/// `root://` URL, which carries its own path — CMS redirectors do this when
/// the replica lives under a different prefix, and treating it as a bare host
/// name asks DNS to resolve the entire URL.
fn redirect_target(hostspec: &str, port: u16, from: &XrdUrl) -> Result<XrdUrl> {
    if XrdUrl::is_url(hostspec) {
        return XrdUrl::parse(hostspec);
    }
    let (host, opaque) = match hostspec.split_once('?') {
        Some((h, o)) => (h, Some(o.to_string())),
        None => (hostspec, None),
    };
    if host.is_empty() {
        return Err(Error::protocol("kXR_redirect named no host"));
    }
    Ok(XrdUrl {
        host: host.to_string(),
        port,
        path: from.path.clone(),
        opaque: opaque.or_else(|| from.opaque.clone()),
    })
}

/// Extracts the size from a stat string of the form "id size flags modtime".
fn parse_stat_size(body: &[u8]) -> Option<u64> {
    cstr(body).split_whitespace().nth(1)?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_xrootd_urls() {
        let u = XrdUrl::parse("root://cms-xrd-global.cern.ch//store/mc/file.root").unwrap();
        assert_eq!(u.host, "cms-xrd-global.cern.ch");
        assert_eq!(u.port, 1094);
        assert_eq!(u.path, "/store/mc/file.root");

        let u = XrdUrl::parse("root://host.example:2094//a/b.root?auth=x").unwrap();
        assert_eq!(u.port, 2094);
        assert_eq!(u.path, "/a/b.root");
        assert_eq!(u.opaque.as_deref(), Some("auth=x"));
    }

    #[test]
    fn rejects_non_xrootd_urls() {
        assert!(XrdUrl::parse("/local/path.root").is_err());
        assert!(XrdUrl::parse("root://host").is_err());
    }

    #[test]
    fn reads_size_from_a_stat_string() {
        assert_eq!(
            parse_stat_size(b"4481113102753988655 17964815 16 1772594332\0"),
            Some(17_964_815)
        );
    }

    fn asked_for() -> XrdUrl {
        XrdUrl::parse("root://redirector.example//store/mc/file.root?first=1").unwrap()
    }

    #[test]
    fn a_redirect_to_a_bare_host_keeps_the_path() {
        let t = redirect_target("data.example", 1095, &asked_for()).unwrap();
        assert_eq!(t.host, "data.example");
        assert_eq!(t.port, 1095);
        assert_eq!(t.path, "/store/mc/file.root");
        assert_eq!(t.opaque.as_deref(), Some("first=1"));
    }

    #[test]
    fn a_redirect_may_replace_the_opaque_data() {
        let t = redirect_target("data.example?token=abc", 1094, &asked_for()).unwrap();
        assert_eq!(t.host, "data.example");
        assert_eq!(t.opaque.as_deref(), Some("token=abc"));
    }

    /// A redirector may answer with a whole URL, whose path replaces ours.
    /// Treating it as a host name asks DNS to resolve the entire URL.
    #[test]
    fn a_redirect_to_a_full_url_takes_its_host_and_path() {
        let t = redirect_target(
            "root://eoscms.cern.ch:1094//eos/cms//store/mc/file.root",
            1094,
            &asked_for(),
        )
        .unwrap();
        assert_eq!(t.host, "eoscms.cern.ch");
        assert_eq!(t.port, 1094);
        assert_eq!(t.path, "/eos/cms//store/mc/file.root");
    }

    #[test]
    fn a_redirect_naming_no_host_is_an_error() {
        assert!(redirect_target("", 1094, &asked_for()).is_err());
    }
}
