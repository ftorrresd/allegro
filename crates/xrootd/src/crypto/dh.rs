//! Diffie-Hellman key agreement for the GSI session cipher.
//!
//! The peer sends PKCS#3 DH parameters as a PEM block, immediately followed by
//! its public value in the delimiters `---BPUB---<hex>---EPUB---`. The shared
//! secret is computed with padding enabled (OpenSSL's `set_dh_pad(1)`), so it
//! is left-padded with zeros to the length of the prime; the session key is
//! the leading bytes of that secret.

use rsa::BigUint;

use super::{bn_to_hex, hex_to_bytes, pem_split, random_bytes};

pub struct DhParams {
    pub p: BigUint,
    pub g: BigUint,
    /// The parameters exactly as the peer sent them, reused verbatim when
    /// echoing our own public value back.
    pub pem_text: String,
}

pub struct DhKeyPair {
    pub params: DhParams,
    pub private: BigUint,
    pub public: BigUint,
}

/// Minimal DER reader for `SEQUENCE { INTEGER p, INTEGER g }`.
fn der_parse_dh(der: &[u8]) -> Result<(BigUint, BigUint), String> {
    let mut pos = 0usize;
    let (seq, _) = der_read_tlv(der, &mut pos, 0x30)?;
    let mut inner = 0usize;
    let (p, _) = der_read_tlv(seq, &mut inner, 0x02)?;
    let (g, _) = der_read_tlv(seq, &mut inner, 0x02)?;
    Ok((BigUint::from_bytes_be(p), BigUint::from_bytes_be(g)))
}

/// Reads one DER TLV of the expected tag, advancing `pos`.
fn der_read_tlv<'a>(
    buf: &'a [u8],
    pos: &mut usize,
    want_tag: u8,
) -> Result<(&'a [u8], u8), String> {
    if *pos + 2 > buf.len() {
        return Err("truncated DER".into());
    }
    let tag = buf[*pos];
    if tag != want_tag {
        return Err(format!(
            "expected DER tag 0x{want_tag:02x}, found 0x{tag:02x}"
        ));
    }
    *pos += 1;
    let first = buf[*pos];
    *pos += 1;
    let len = if first & 0x80 == 0 {
        first as usize
    } else {
        let n = (first & 0x7f) as usize;
        if n == 0 || n > 4 || *pos + n > buf.len() {
            return Err("unsupported DER length".into());
        }
        let mut v = 0usize;
        for _ in 0..n {
            v = (v << 8) | buf[*pos] as usize;
            *pos += 1;
        }
        v
    };
    if *pos + len > buf.len() {
        return Err("DER length runs past the buffer".into());
    }
    let body = &buf[*pos..*pos + len];
    *pos += len;
    Ok((body, tag))
}

/// Parses the peer's key-agreement buffer into DH parameters and its public
/// value.
pub fn parse_peer_public(buf: &[u8]) -> Result<(DhParams, BigUint), String> {
    let text = String::from_utf8_lossy(buf);
    let bpub = text
        .find("---BPUB---")
        .ok_or("key-agreement buffer has no ---BPUB--- marker")?;
    let epub = text
        .find("---EPUB--")
        .ok_or("key-agreement buffer has no ---EPUB--- marker")?;
    let hex = text[bpub + 10..epub].trim();
    let peer =
        BigUint::from_bytes_be(&hex_to_bytes(hex).ok_or("peer public value is not valid hex")?);

    let pem_text = text[..bpub].to_string();
    let blocks = pem_split(pem_text.as_bytes())?;
    let params = blocks
        .iter()
        .find(|b| b.label.contains("DH PARAMETERS"))
        .ok_or("no DH PARAMETERS block in key-agreement buffer")?;
    let (p, g) = der_parse_dh(&params.der)?;
    Ok((DhParams { p, g, pem_text }, peer))
}

impl DhKeyPair {
    /// Generates a fresh private value and the matching public value.
    pub fn generate(params: DhParams) -> Self {
        let nbytes = params.p.to_bytes_be().len();
        // A private exponent the size of the prime, reduced into range.
        let mut x = BigUint::from_bytes_be(&random_bytes(nbytes));
        let two = BigUint::from(2u32);
        let limit = &params.p - &two;
        x %= &limit;
        if x < two {
            x = two.clone();
        }
        let public = params.g.modpow(&x, &params.p);
        Self {
            params,
            private: x,
            public,
        }
    }

    /// The shared secret, left-padded with zeros to the length of the prime,
    /// matching OpenSSL's padded derivation.
    pub fn shared_secret(&self, peer: &BigUint) -> Vec<u8> {
        let z = peer.modpow(&self.private, &self.params.p);
        let plen = self.params.p.to_bytes_be().len();
        let mut out = z.to_bytes_be();
        if out.len() < plen {
            let mut padded = vec![0u8; plen - out.len()];
            padded.extend_from_slice(&out);
            out = padded;
        }
        out
    }

    /// Our public value in the peer's expected format: the DH parameter PEM
    /// followed by `---BPUB---<hex>---EPUB---`.
    pub fn public_buffer(&self) -> Vec<u8> {
        let mut s = self.params.pem_text.clone();
        s.push_str("---BPUB---");
        s.push_str(&bn_to_hex(&self.public.to_bytes_be()));
        s.push_str("---EPUB---");
        s.into_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // A small PKCS#3 parameter set is enough to check the wire format and the
    // agreement property.
    fn sample_params() -> String {
        // p = 2^127 - 1 (prime), g = 2, DER encoded then PEM wrapped.
        let p_bytes = {
            let mut v = vec![0x7f];
            v.extend_from_slice(&[0xffu8; 15]);
            v
        };
        let mut inner = Vec::new();
        inner.push(0x02);
        inner.push(p_bytes.len() as u8);
        inner.extend_from_slice(&p_bytes);
        inner.extend_from_slice(&[0x02, 0x01, 0x02]);
        let mut der = vec![0x30, inner.len() as u8];
        der.extend_from_slice(&inner);
        super::super::pem_encode("DH PARAMETERS", &der)
    }

    #[test]
    fn both_sides_agree_on_the_shared_secret() {
        let pem = sample_params();
        let mut a_buf = pem.clone().into_bytes();
        a_buf.extend_from_slice(b"---BPUB---02---EPUB---");
        let (params_a, _) = parse_peer_public(&a_buf).unwrap();
        let a = DhKeyPair::generate(params_a);

        let (params_b, _) = parse_peer_public(&a_buf).unwrap();
        let b = DhKeyPair::generate(params_b);

        assert_eq!(a.shared_secret(&b.public), b.shared_secret(&a.public));
    }

    #[test]
    fn public_buffer_round_trips() {
        let pem = sample_params();
        let mut buf = pem.into_bytes();
        buf.extend_from_slice(b"---BPUB---02---EPUB---");
        let (params, peer) = parse_peer_public(&buf).unwrap();
        assert_eq!(peer, BigUint::from(2u32));
        let kp = DhKeyPair::generate(params);
        let (_, reparsed) = parse_peer_public(&kp.public_buffer()).unwrap();
        assert_eq!(reparsed, kp.public);
    }
}
