//! TLS transport for XRootD connections.
//!
//! XRootD 5 servers set `kXR_tlsLogin`, meaning the session starts in the
//! clear (handshake + `kXR_protocol`) and must switch to TLS immediately
//! before `kXR_login`. So TLS is layered onto an already-connected socket
//! rather than established at connect time.
//!
//! The TLS state machine is rustls, driven by a cryptography provider built
//! entirely from RustCrypto crates, so nothing here links against C.

use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::sync::Arc;

use crate::error::Error;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, ClientConnection, DigitallySignedStruct, SignatureScheme, StreamOwned};

pub struct TlsStream {
    inner: StreamOwned<ClientConnection, TcpStream>,
}

impl TlsStream {
    /// Upgrades an established plaintext socket to TLS.
    pub fn upgrade(sock: TcpStream, host: &str) -> crate::error::Result<Self> {
        let provider = Arc::new(rustls_rustcrypto::provider());
        let verifier = Arc::new(GsiTransportVerifier {
            provider: provider.clone(),
        });

        let config = ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(|e| Error::tls(format!("selecting protocol versions: {e}")))?
            .dangerous()
            .with_custom_certificate_verifier(verifier)
            .with_no_client_auth();

        // The server name is only used for SNI here; the GSI layer performs
        // the authoritative check of the server's X.509 identity.
        let name = ServerName::try_from(host.to_string())
            .map_err(|e| Error::tls(format!("invalid server name {host}: {e}")))?;
        let conn = ClientConnection::new(Arc::new(config), name)
            .map_err(|e| Error::tls(format!("initialising client connection: {e}")))?;

        let mut stream = StreamOwned::new(conn, sock);
        // Drive the handshake to completion now so failures surface here
        // rather than inside the first XRootD request.
        while stream.conn.is_handshaking() {
            stream
                .conn
                .complete_io(&mut stream.sock)
                .map_err(|e| Error::tls(format!("handshake with {host} failed: {e}")))?;
        }
        Ok(Self { inner: stream })
    }

    /// The peer certificate chain, as presented during the handshake.
    pub fn peer_certificates(&self) -> Option<Vec<Vec<u8>>> {
        self.inner
            .conn
            .peer_certificates()
            .map(|cs| cs.iter().map(|c| c.as_ref().to_vec()).collect())
    }
}

impl Read for TlsStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf)
    }
}

impl Write for TlsStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// Certificate verification for the XRootD TLS wrapper.
///
/// The grid trust decision is made by the GSI layer, which validates the
/// server's certificate chain against the IGTF CA store and checks that the
/// certificate actually names the host being contacted. This verifier checks
/// the handshake signatures (proving possession of the private key) and
/// defers chain trust to that layer.
#[derive(Debug)]
struct GsiTransportVerifier {
    provider: Arc<rustls::crypto::CryptoProvider>,
}

impl ServerCertVerifier for GsiTransportVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}
