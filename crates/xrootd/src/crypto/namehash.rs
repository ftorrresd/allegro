//! OpenSSL-compatible X.509 name hashes.
//!
//! GSI identifies certificate authorities by the same 8-hex-digit hashes that
//! `openssl x509 -hash` prints and that name the files in
//! `/etc/grid-security/certificates`. Two flavours exist and XRootD sends both,
//! separated by `|`:
//!
//! * the current hash: first four bytes of `SHA1` over the *canonical* DER
//!   encoding of the name, read as a little-endian `u32`;
//! * the legacy hash: the same, but `MD5` over the name's raw DER.
//!
//! Canonicalisation (OpenSSL's `X509_NAME_canon`) re-encodes every attribute
//! value as a `UTF8String` whose content is lower-cased, has leading and
//! trailing whitespace removed and internal whitespace runs collapsed to a
//! single space.

use der::{Decode, Encode};
use md5::Md5;
use sha1::{Digest, Sha1};
use x509_cert::name::Name;

/// Encodes a DER tag/length/value triple.
fn der_tlv(tag: u8, body: &[u8]) -> Vec<u8> {
    let mut out = vec![tag];
    let len = body.len();
    if len < 0x80 {
        out.push(len as u8);
    } else {
        let bytes = len.to_be_bytes();
        let first = bytes
            .iter()
            .position(|&b| b != 0)
            .unwrap_or(bytes.len() - 1);
        let sig = &bytes[first..];
        out.push(0x80 | sig.len() as u8);
        out.extend_from_slice(sig);
    }
    out.extend_from_slice(body);
    out
}

/// Applies OpenSSL's `asn1_string_canon` transformation.
fn canon_string(value: &[u8]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::with_capacity(value.len());
    let mut pending_space = false;
    for &b in value {
        if b.is_ascii_whitespace() {
            // Whitespace is only emitted between non-space characters, which
            // collapses runs and drops leading/trailing spaces.
            pending_space = !out.is_empty();
            continue;
        }
        if pending_space {
            out.push(b' ');
            pending_space = false;
        }
        out.push(b.to_ascii_lowercase());
    }
    out
}

/// Builds the canonical DER encoding of a name.
///
/// Note the absence of an outer `SEQUENCE`: OpenSSL's `i2d_name_canon`
/// concatenates the encodings of the individual RDN `SET`s, and the hash is
/// taken over exactly those bytes.
fn canonical_der(name: &Name) -> Result<Vec<u8>, String> {
    let mut rdns_der = Vec::new();
    for rdn in name.0.iter() {
        let mut avas: Vec<Vec<u8>> = Vec::new();
        for ava in rdn.0.iter() {
            let oid_der = ava
                .oid
                .to_der()
                .map_err(|e| format!("encoding attribute type: {e}"))?;
            let raw = ava.value.value().to_vec();
            // Every string-valued attribute becomes a canonicalised UTF8String.
            let value_der = der_tlv(0x0c, &canon_string(&raw));
            let mut body = oid_der;
            body.extend_from_slice(&value_der);
            avas.push(der_tlv(0x30, &body));
        }
        // DER requires the members of a SET OF to be sorted by their encoding.
        avas.sort();
        let mut set_body = Vec::new();
        for a in &avas {
            set_body.extend_from_slice(a);
        }
        rdns_der.extend_from_slice(&der_tlv(0x31, &set_body));
    }
    Ok(rdns_der)
}

fn first4_le(digest: &[u8]) -> u32 {
    u32::from_le_bytes([digest[0], digest[1], digest[2], digest[3]])
}

/// The current-style hash, as `openssl x509 -hash` prints it.
pub fn name_hash(name: &Name) -> Result<String, String> {
    let der = canonical_der(name)?;
    let digest = Sha1::digest(&der);
    Ok(format!("{:08x}", first4_le(&digest)))
}

/// The legacy MD5-based hash, as `openssl x509 -subject_hash_old` prints it.
pub fn name_hash_old(name: &Name) -> Result<String, String> {
    let der = name
        .to_der()
        .map_err(|e| format!("re-encoding name: {e}"))?;
    let digest = Md5::digest(&der);
    Ok(format!("{:08x}", first4_le(&digest)))
}

/// Both hashes joined by `|`, the form XRootD puts in the `kXRS_issuer_hash`
/// bucket when hash compatibility is enabled.
pub fn name_hash_pair(name: &Name) -> Result<String, String> {
    let new = name_hash(name)?;
    let old = name_hash_old(name)?;
    if new == old {
        Ok(new)
    } else {
        Ok(format!("{new}|{old}"))
    }
}

/// Parses a DER name, for tests and for CA store indexing.
pub fn name_from_der(der: &[u8]) -> Result<Name, String> {
    Name::from_der(der).map_err(|e| format!("parsing name: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use x509_cert::Certificate;

    /// Ground truth captured from `openssl x509` on the live proxy chain:
    /// the user certificate's issuer is the CERN Grid CA.
    #[test]
    fn matches_openssl_hashes_for_the_proxy_chain() {
        let path = "/tmp/x509up_u1001";
        let Ok(data) = std::fs::read(path) else {
            eprintln!("skipping: {path} not present");
            return;
        };
        let blocks = crate::crypto::pem_split(&data).unwrap();
        let certs: Vec<Certificate> = blocks
            .iter()
            .filter(|b| b.label == "CERTIFICATE")
            .map(|b| Certificate::from_der(&b.der).unwrap())
            .collect();
        assert_eq!(certs.len(), 2, "expected proxy + user certificate");

        // Proxy certificate: subject/issuer hashes.
        assert_eq!(
            name_hash(&certs[0].tbs_certificate.subject).unwrap(),
            "1b0fc7a5"
        );
        assert_eq!(
            name_hash(&certs[0].tbs_certificate.issuer).unwrap(),
            "8ef0a748"
        );
        assert_eq!(
            name_hash_old(&certs[0].tbs_certificate.subject).unwrap(),
            "0c3fc8cd"
        );
        assert_eq!(
            name_hash_old(&certs[0].tbs_certificate.issuer).unwrap(),
            "8a50dc6a"
        );

        // User certificate: its issuer is the CA we must advertise to the peer.
        assert_eq!(
            name_hash(&certs[1].tbs_certificate.subject).unwrap(),
            "8ef0a748"
        );
        assert_eq!(
            name_hash(&certs[1].tbs_certificate.issuer).unwrap(),
            "5168735f"
        );
        assert_eq!(
            name_hash_old(&certs[1].tbs_certificate.issuer).unwrap(),
            "4339b4bc"
        );
    }
}
