//! GSI authentication (`XrdSecProtocolgsi`) for the XRootD client.
//!
//! The exchange is:
//!
//! 1. client → `kXGC_certreq`: our crypto module, protocol version, the hash
//!    of the CA that issued our credential, and a random tag for the server
//!    to sign;
//! 2. server → `kXGS_cert`: its certificate, its Diffie-Hellman parameters and
//!    public value signed with its private key, our tag signed back, and a
//!    fresh tag for us;
//! 3. client → `kXGC_cert`: our proxy chain, our DH public value signed with
//!    the proxy key, and the server's tag signed back — everything in the main
//!    bucket encrypted under the freshly agreed session key;
//! 4. server → `kXR_ok`.
//!
//! Proving possession of the proxy private key (by signing the server's random
//! tag) is what authenticates us.

pub mod bucket;
pub mod proxy;

use der::Encode;
use rsa::pkcs8::DecodePublicKey;
use rsa::RsaPublicKey;
use x509_cert::Certificate;

use crate::gsi::bucket::*;
use crate::gsi::proxy::ProxyCredential;

use crate::crypto::{
    aes128_cbc_encrypt_with_iv, dh::parse_peer_public, dh::DhKeyPair, pem_split, random_tag,
    rsa_raw,
};
use crate::error::{Context, Error, Result};

/// The GSI protocol version this client implements.
const GSI_VERSION: i32 = 10600;
/// Version from which DH parameters are signed and a per-message IV is used.
const GSI_VERSION_DH_SIGNED: i32 = 10400;

/// What the server advertised in its security token.
#[derive(Debug, Clone)]
pub struct GsiParams {
    pub version: i32,
    pub crypto_list: String,
    pub ca_list: String,
}

impl GsiParams {
    /// Parses `&P=gsi,v:10600,c:ssl,ca:1e49ade3.0|3cbc995f.0&P=ztn,...`.
    pub fn parse(sec_token: &str) -> Result<Self> {
        let section = sec_token
            .split('&')
            .find_map(|s| s.strip_prefix("P=gsi,"))
            .ok_or_else(|| {
                Error::auth(format!("server does not offer gsi (token: {sec_token})"))
            })?;
        let mut version = GSI_VERSION;
        let mut crypto_list = String::from("ssl");
        let mut ca_list = String::new();
        for field in section.split(',') {
            if let Some(v) = field.strip_prefix("v:") {
                version = v.trim().parse().unwrap_or(GSI_VERSION);
            } else if let Some(c) = field.strip_prefix("c:") {
                crypto_list = c.trim().to_string();
            } else if let Some(ca) = field.strip_prefix("ca:") {
                ca_list = ca.trim().to_string();
            }
        }
        Ok(Self {
            version,
            crypto_list,
            ca_list,
        })
    }
}

pub struct GsiAuthenticator {
    proxy: ProxyCredential,
    params: GsiParams,
    /// Random tag we last sent; the server must return it signed.
    pending_rtag: Option<Vec<u8>>,
    session_key: Option<Vec<u8>>,
    /// Server certificate, retained so the caller can check its identity.
    pub server_cert: Option<Certificate>,
    step: i32,
}

impl GsiAuthenticator {
    pub fn new(proxy: ProxyCredential, params: GsiParams) -> Self {
        Self {
            proxy,
            params,
            pending_rtag: None,
            session_key: None,
            server_cert: None,
            step: KXGS_INIT,
        }
    }

    fn uses_signed_dh(&self) -> bool {
        self.params.version >= GSI_VERSION_DH_SIGNED
    }

    /// Builds the `kXGC_certreq` credentials that open the exchange.
    pub fn initial_credentials(&mut self) -> Result<Vec<u8>> {
        let mut bpar = SutBuffer::new(KXGC_CERTREQ);
        bpar.add_str(KXRS_CRYPTOMOD, "ssl");
        bpar.add_i32(KXRS_VERSION, GSI_VERSION);
        bpar.add_str(KXRS_ISSUER_HASH, &self.proxy.issuer_hash);
        if self.params.version >= 10100 {
            // No proxy delegation is requested.
            bpar.add_i32(KXRS_CLNT_OPTS, 0);
        }

        let mut bmai = SutBuffer::new(KXGC_CERTREQ);
        let tag = random_tag();
        self.pending_rtag = Some(tag.as_bytes().to_vec());
        bmai.add_str(KXRS_RTAG, &tag);
        bpar.add(KXRS_MAIN, bmai.serialize());

        self.step = KXGC_CERTREQ;
        Ok(bpar.serialize())
    }

    /// Consumes a server message and produces the next client message.
    ///
    /// Returns `None` when the server has nothing further to answer.
    pub fn next_credentials(&mut self, server_msg: &[u8]) -> Result<Option<Vec<u8>>> {
        let bpar = SutBuffer::parse(server_msg).context("parsing GSI server message")?;
        if bpar.protocol != "gsi" {
            return Err(Error::auth(format!(
                "expected a gsi message, got protocol {:?}",
                bpar.protocol
            )));
        }
        if let Some(msg) = bpar.get_string(KXRS_MESSAGE) {
            if !msg.is_empty() {
                return Err(Error::auth(format!("server reported: {msg}")));
            }
        }
        match bpar.step {
            KXGS_CERT => self.handle_server_cert(bpar).map(Some),
            other => Err(Error::auth(format!(
                "unexpected GSI step {other} from server"
            ))),
        }
    }

    /// Handles `kXGS_cert` and produces `kXGC_cert`.
    fn handle_server_cert(&mut self, mut bpar: SutBuffer) -> Result<Vec<u8>> {
        // --- the server's certificate and public key ---------------------
        let x509 = bpar
            .get(KXRS_X509)
            .ok_or_else(|| Error::auth("server did not send its certificate"))?;
        let blocks = pem_split(x509).context("parsing server certificate PEM")?;
        let cert_der = blocks
            .iter()
            .find(|b| b.label == "CERTIFICATE")
            .ok_or_else(|| Error::auth("no CERTIFICATE block from server"))?;
        let cert = <Certificate as der::Decode>::from_der(&cert_der.der)
            .map_err(|e| Error::auth(format!("parsing server certificate: {e}")))?;
        let spki = cert
            .tbs_certificate
            .subject_public_key_info
            .to_der()
            .map_err(|e| Error::auth(format!("re-encoding server public key: {e}")))?;
        let server_key = RsaPublicKey::from_public_key_der(&spki)
            .map_err(|e| Error::auth(format!("server key is not usable RSA: {e}")))?;
        self.server_cert = Some(cert);

        // --- negotiate the symmetric algorithms --------------------------
        let cipher_list = bpar.get_string(KXRS_CIPHER_ALG);
        let cipher = pick_from_list(cipher_list.as_deref(), "aes-128-cbc")
            .ok_or_else(|| Error::auth("no cipher in common with the server"))?;
        if cipher != "aes-128-cbc" {
            return Err(Error::auth(format!(
                "server requires cipher {cipher}, only aes-128-cbc is implemented"
            )));
        }
        let digest_list = bpar.get_string(KXRS_MD_ALG);
        let digest = pick_from_list(digest_list.as_deref(), "sha256").unwrap_or("sha256");

        // --- Diffie-Hellman key agreement --------------------------------
        // Newer servers sign their DH public part with the certificate key,
        // which both authenticates it and binds it to the certificate.
        let dh_material = if self.uses_signed_dh() {
            let signed = bpar
                .get(KXRS_CIPHER)
                .ok_or_else(|| Error::auth("server sent no DH public part"))?;
            rsa_raw::decrypt_public(&server_key, signed)
                .context("verifying the server's signed DH parameters")?
        } else {
            bpar.get(KXRS_PUK)
                .ok_or_else(|| Error::auth("server sent no DH public part"))?
                .to_vec()
        };

        let (params, peer_public) =
            parse_peer_public(&dh_material).context("reading the server's DH parameters")?;
        let keypair = DhKeyPair::generate(params);
        let secret = keypair.shared_secret(&peer_public);
        // OpenSSL keys AES-128-CBC from the leading bytes of the shared secret.
        let session_key = secret[..16].to_vec();
        self.session_key = Some(session_key.clone());

        // --- check the server signed the tag we sent ----------------------
        let main = bpar
            .get(KXRS_MAIN)
            .ok_or_else(|| Error::auth("server sent no main bucket"))?;
        let bmai_in = SutBuffer::parse(main).context("parsing the server's main bucket")?;
        let expected = self
            .pending_rtag
            .clone()
            .ok_or_else(|| Error::auth("no outstanding random tag"))?;
        let signed_rtag = bmai_in
            .get(KXRS_SIGNED_RTAG)
            .ok_or_else(|| Error::auth("server did not sign our random tag"))?;
        let recovered = rsa_raw::decrypt_public(&server_key, signed_rtag)
            .context("verifying the server's signature over our random tag")?;
        if recovered != expected {
            return Err(Error::auth(
                "server's signature does not match the tag we sent",
            ));
        }

        // --- build our reply ---------------------------------------------
        let mut bmai = SutBuffer::new(KXGC_CERT);
        // Sign the tag the server just gave us: this proves we hold the proxy
        // private key, and is the actual authentication step.
        if let Some(their_tag) = bmai_in.get(KXRS_RTAG) {
            let signed = rsa_raw::encrypt_private(&self.proxy.key, their_tag)
                .context("signing the server's random tag")?;
            bmai.add(KXRS_SIGNED_RTAG, signed);
        }
        // A fresh tag for the next leg, if there is one.
        let tag = random_tag();
        self.pending_rtag = Some(tag.as_bytes().to_vec());
        bmai.add_str(KXRS_RTAG, &tag);
        bmai.add(KXRS_X509, self.proxy.chain_pem.clone());

        let encrypted_main = aes128_cbc_encrypt_with_iv(&session_key, &bmai.serialize())
            .context("encrypting the GSI main bucket")?;

        // The reply reuses the server's buffer with the agreed choices.
        bpar.step = KXGC_CERT;
        bpar.remove(KXRS_X509);
        bpar.remove(KXRS_CIPHER);
        bpar.set_str(
            KXRS_CIPHER_ALG,
            &format!("{cipher}#{}", crate::crypto::AES128_IV_LEN),
        );
        bpar.set_str(KXRS_MD_ALG, digest);

        let our_public = keypair.public_buffer();
        let signed_public = rsa_raw::encrypt_private(&self.proxy.key, &our_public)
            .context("signing our DH public value")?;
        bpar.add(KXRS_CIPHER, signed_public);
        bpar.set(KXRS_PUK, self.proxy.public_key_pem()?);
        bpar.set(KXRS_MAIN, encrypted_main);

        self.step = KXGC_CERT;
        Ok(bpar.serialize())
    }
}

/// Chooses `preferred` from a colon-separated list if present, else the first
/// entry.
fn pick_from_list<'a>(list: Option<&'a str>, preferred: &'a str) -> Option<&'a str> {
    let mut items = list?.split(':').map(str::trim).filter(|s| !s.is_empty());
    let first = items.next()?;
    if first == preferred || items.any(|item| item == preferred) {
        Some(preferred)
    } else {
        Some(first)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_advertised_security_token() {
        let token = "&P=gsi,v:10600,c:ssl,ca:1e49ade3.0|3cbc995f.0&P=ztn,0:4096:";
        let p = GsiParams::parse(token).unwrap();
        assert_eq!(p.version, 10600);
        assert_eq!(p.crypto_list, "ssl");
        assert_eq!(p.ca_list, "1e49ade3.0|3cbc995f.0");
    }

    #[test]
    fn rejects_a_token_without_gsi() {
        assert!(GsiParams::parse("&P=ztn,0:4096:").is_err());
    }

    #[test]
    fn prefers_aes_from_the_cipher_list() {
        assert_eq!(
            pick_from_list(Some("bf-cbc:aes-128-cbc"), "aes-128-cbc").unwrap(),
            "aes-128-cbc"
        );
        assert_eq!(
            pick_from_list(Some("bf-cbc"), "aes-128-cbc").unwrap(),
            "bf-cbc"
        );
    }
}
