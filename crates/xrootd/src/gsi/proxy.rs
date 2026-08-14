//! Loading the user's X.509 proxy credential.
//!
//! A grid proxy file holds, in order, the proxy certificate, its private key
//! and the certificates issuing it, all PEM encoded in one file.

use std::path::{Path, PathBuf};

use der::{Decode, Encode};
use rsa::pkcs1::DecodeRsaPrivateKey;
use rsa::pkcs8::{DecodePrivateKey, EncodePublicKey};
use rsa::{RsaPrivateKey, RsaPublicKey};
use x509_cert::Certificate;

use crate::crypto::{namehash, pem_encode, pem_split};
use crate::error::{Context, Error, Result};

pub struct ProxyCredential {
    /// Certificates ordered leaf first, then each issuer in turn, stopping
    /// before any self-signed CA — the order GSI expects on the wire.
    pub chain: Vec<Certificate>,
    /// PEM of `chain`, the payload of the `kXRS_x509` bucket.
    pub chain_pem: Vec<u8>,
    pub key: RsaPrivateKey,
    /// Hash pair of the CA that issued the topmost certificate in the chain.
    pub issuer_hash: String,
    pub path: PathBuf,
}

/// Locates the proxy: `X509_USER_PROXY` if set, else `/tmp/x509up_u<uid>`.
pub fn default_proxy_path() -> Result<PathBuf> {
    if let Ok(p) = std::env::var("X509_USER_PROXY") {
        if !p.is_empty() {
            return Ok(PathBuf::from(p));
        }
    }
    // Reading our own uid without libc: /proc/self is owned by us.
    use std::os::unix::fs::MetadataExt;
    let uid = std::fs::metadata("/proc/self")
        .context("determining the current uid from /proc/self")?
        .uid();
    Ok(PathBuf::from(format!("/tmp/x509up_u{uid}")))
}

fn is_self_signed(c: &Certificate) -> bool {
    c.tbs_certificate.subject == c.tbs_certificate.issuer
}

impl ProxyCredential {
    pub fn load(path: &Path) -> Result<Self> {
        let data =
            std::fs::read(path).context(format!("reading proxy credential {}", path.display()))?;
        let blocks = pem_split(&data).context("parsing proxy PEM")?;

        let mut certs = Vec::new();
        let mut key = None;
        for b in &blocks {
            match b.label.as_str() {
                "CERTIFICATE" => certs.push(
                    Certificate::from_der(&b.der)
                        .map_err(|e| Error::crypto(format!("parsing proxy certificate: {e}")))?,
                ),
                "RSA PRIVATE KEY" => {
                    key = Some(RsaPrivateKey::from_pkcs1_der(&b.der).map_err(|e| {
                        Error::crypto(format!("parsing PKCS#1 proxy private key: {e}"))
                    })?)
                }
                "PRIVATE KEY" => {
                    key = Some(RsaPrivateKey::from_pkcs8_der(&b.der).map_err(|e| {
                        Error::crypto(format!("parsing PKCS#8 proxy private key: {e}"))
                    })?)
                }
                _ => {}
            }
        }

        let key = key.ok_or_else(|| {
            Error::crypto(format!(
                "no private key in {} — is this a proxy file?",
                path.display()
            ))
        })?;
        if certs.is_empty() {
            return Err(Error::crypto(format!(
                "no certificates in {}",
                path.display()
            )));
        }

        // The leaf is the certificate that issues nothing else in the file.
        let leaf_idx = (0..certs.len())
            .find(|&i| {
                !certs.iter().enumerate().any(|(j, c)| {
                    j != i && c.tbs_certificate.issuer == certs[i].tbs_certificate.subject
                })
            })
            .ok_or_else(|| Error::crypto("proxy chain has no leaf certificate"))?;

        // Walk from the leaf up to (but excluding) any self-signed CA.
        let mut chain = Vec::new();
        let mut current = leaf_idx;
        let mut seen = vec![false; certs.len()];
        loop {
            if seen[current] {
                break;
            }
            seen[current] = true;
            if is_self_signed(&certs[current]) && !chain.is_empty() {
                break;
            }
            chain.push(certs[current].clone());
            let issuer = certs[current].tbs_certificate.issuer.clone();
            match certs
                .iter()
                .position(|c| c.tbs_certificate.subject == issuer && !is_self_signed(c))
            {
                Some(next) if next != current => current = next,
                _ => break,
            }
        }

        // The CA to advertise is the issuer of the topmost certificate held.
        let top = chain.last().expect("chain is non-empty");
        let issuer_hash = namehash::name_hash_pair(&top.tbs_certificate.issuer)
            .context("hashing the issuing CA name")?;

        let mut chain_pem = Vec::new();
        for c in &chain {
            let der = c
                .to_der()
                .map_err(|e| Error::crypto(format!("re-encoding certificate: {e}")))?;
            chain_pem.extend_from_slice(pem_encode("CERTIFICATE", &der).as_bytes());
        }

        Ok(Self {
            chain,
            chain_pem,
            key,
            issuer_hash,
            path: path.to_path_buf(),
        })
    }

    pub fn public_key(&self) -> RsaPublicKey {
        RsaPublicKey::from(&self.key)
    }

    /// The signing key's public half in the PEM `SubjectPublicKeyInfo` form
    /// that `PEM_write_bio_PUBKEY` produces, for the `kXRS_puk` bucket.
    pub fn public_key_pem(&self) -> Result<Vec<u8>> {
        let der = self
            .public_key()
            .to_public_key_der()
            .map_err(|e| Error::crypto(format!("encoding public key: {e}")))?;
        Ok(pem_encode("PUBLIC KEY", der.as_bytes()).into_bytes())
    }

    /// Subject of the proxy certificate, for logging.
    pub fn subject(&self) -> String {
        self.chain
            .first()
            .map(|c| c.tbs_certificate.subject.to_string())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_the_live_proxy_credential() {
        let Ok(path) = default_proxy_path() else {
            return;
        };
        if !path.exists() {
            eprintln!("skipping: no proxy at {}", path.display());
            return;
        }
        let p = ProxyCredential::load(&path).expect("proxy should load");
        assert!(!p.chain.is_empty());
        // Leaf first: the proxy certificate is issued by the user certificate.
        assert!(p.chain.len() >= 2, "expected proxy + user certificate");
        assert_eq!(
            p.chain[0].tbs_certificate.issuer,
            p.chain[1].tbs_certificate.subject
        );
        // The advertised CA is the issuer of the user certificate.
        assert_eq!(p.issuer_hash, "5168735f|4339b4bc");
        assert!(p.chain_pem.starts_with(b"-----BEGIN CERTIFICATE-----"));
        assert!(p
            .public_key_pem()
            .unwrap()
            .starts_with(b"-----BEGIN PUBLIC KEY-----"));
    }
}
