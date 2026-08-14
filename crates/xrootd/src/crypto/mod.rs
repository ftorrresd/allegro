//! Cryptographic primitives needed by the GSI authentication handshake,
//! built from RustCrypto crates only.

pub mod dh;
pub mod namehash;
pub mod rsa_raw;

use aes::cipher::{block_padding::Pkcs7, BlockDecryptMut, BlockEncryptMut, KeyIvInit};
use rand::RngCore;

/// One PEM block: its label and decoded DER contents.
pub struct PemBlock {
    pub label: String,
    pub der: Vec<u8>,
}

/// Splits a buffer holding one or more concatenated PEM blocks.
///
/// Proxy files hold the proxy certificate, its private key and the issuing
/// certificates back to back, so a splitting reader is needed rather than a
/// single-block decoder.
pub fn pem_split(data: &[u8]) -> Result<Vec<PemBlock>, String> {
    let text = String::from_utf8_lossy(data);
    let mut out = Vec::new();
    let mut rest = text.as_ref();

    while let Some(begin) = rest.find("-----BEGIN ") {
        let after = &rest[begin..];
        let label_end = after.find("-----\n").or_else(|| after.find("-----\r\n"));
        let label_end = match label_end {
            Some(e) => e,
            None => break,
        };
        let label = after[11..label_end].to_string();
        let end_marker = format!("-----END {}-----", label);
        let end = match after.find(&end_marker) {
            Some(e) => e + end_marker.len(),
            None => return Err(format!("unterminated PEM block {label}")),
        };
        let block = &after[..end];
        let der = pem_decode_block(block, &label)?;
        out.push(PemBlock { label, der });
        rest = &after[end..];
    }
    Ok(out)
}

fn pem_decode_block(block: &str, label: &str) -> Result<Vec<u8>, String> {
    let begin = format!("-----BEGIN {}-----", label);
    let end = format!("-----END {}-----", label);
    let body = block
        .trim()
        .strip_prefix(&begin)
        .and_then(|s| s.strip_suffix(&end))
        .ok_or_else(|| format!("malformed PEM block {label}"))?;
    // Skip any RFC 1421 headers (e.g. "Proc-Type:"), then base64-decode.
    let mut b64 = String::new();
    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() || line.contains(':') {
            continue;
        }
        b64.push_str(line);
    }
    base64_decode(&b64).ok_or_else(|| format!("bad base64 in PEM block {label}"))
}

/// Encodes DER as a PEM block with 64-character lines, matching the layout
/// OpenSSL's `PEM_write_bio_*` produces (which GSI peers parse verbatim).
pub fn pem_encode(label: &str, der: &[u8]) -> String {
    let mut s = format!("-----BEGIN {}-----\n", label);
    let b64 = base64_encode(der);
    for chunk in b64.as_bytes().chunks(64) {
        s.push_str(&String::from_utf8_lossy(chunk));
        s.push('\n');
    }
    s.push_str(&format!("-----END {}-----\n", label));
    s
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn base64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(B64[(n >> 18) as usize & 63] as char);
        out.push(B64[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            B64[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            B64[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

pub fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let mut acc: u32 = 0;
    let mut bits = 0u32;
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    for c in s.bytes() {
        if c == b'=' || c == b'\n' || c == b'\r' || c == b' ' {
            continue;
        }
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        } as u32;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

/// Uppercase hex without leading zeros, matching OpenSSL's `BN_bn2hex`
/// (which always emits an even number of digits).
pub fn bn_to_hex(bytes: &[u8]) -> String {
    let first = bytes.iter().position(|&b| b != 0).unwrap_or(bytes.len());
    let sig = &bytes[first..];
    if sig.is_empty() {
        return "00".into();
    }
    let mut s = String::with_capacity(sig.len() * 2);
    for b in sig {
        s.push_str(&format!("{:02X}", b));
    }
    s
}

pub fn hex_to_bytes(s: &str) -> Option<Vec<u8>> {
    let s = if s.len() % 2 == 1 {
        format!("0{s}")
    } else {
        s.to_string()
    };
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

pub fn random_bytes(n: usize) -> Vec<u8> {
    let mut v = vec![0u8; n];
    rand::thread_rng().fill_bytes(&mut v);
    v
}

/// Random bytes whose count is known at compile time.
pub fn random_array<const N: usize>() -> [u8; N] {
    let mut v = [0u8; N];
    rand::thread_rng().fill_bytes(&mut v);
    v
}

/// An 8-character tag over `[a-zA-Z0-9./]`, the format `XrdSutRndm::GetRndmTag`
/// produces and the peer echoes back signed.
pub fn random_tag() -> String {
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789./";
    let raw = random_bytes(8);
    raw.iter()
        .map(|b| ALPHABET[(*b as usize) % ALPHABET.len()] as char)
        .collect()
}

type Aes128CbcEnc = cbc::Encryptor<aes::Aes128>;
type Aes128CbcDec = cbc::Decryptor<aes::Aes128>;

/// AES-128-CBC block size and IV length, as reported by OpenSSL for the
/// cipher GSI negotiates.
pub const AES128_IV_LEN: usize = 16;

/// Encrypts a GSI bucket payload: a fresh random IV followed by the
/// PKCS#7-padded ciphertext, as `XrdCryptoCipher::Encrypt(bucket, useiv)` does.
pub fn aes128_cbc_encrypt_with_iv(key: &[u8], data: &[u8]) -> Result<Vec<u8>, String> {
    if key.len() != 16 {
        return Err(format!("aes-128 needs a 16-byte key, got {}", key.len()));
    }
    let iv = random_bytes(AES128_IV_LEN);
    let enc = Aes128CbcEnc::new(key.into(), iv.as_slice().into());
    let mut out = iv.clone();
    out.extend_from_slice(&enc.encrypt_padded_vec_mut::<Pkcs7>(data));
    Ok(out)
}

/// Inverse of [`aes128_cbc_encrypt_with_iv`]: strips the leading IV and
/// removes PKCS#7 padding.
pub fn aes128_cbc_decrypt_with_iv(key: &[u8], data: &[u8]) -> Result<Vec<u8>, String> {
    if key.len() != 16 {
        return Err(format!("aes-128 needs a 16-byte key, got {}", key.len()));
    }
    if data.len() <= AES128_IV_LEN {
        return Err("ciphertext shorter than its IV".into());
    }
    let (iv, body) = data.split_at(AES128_IV_LEN);
    let dec = Aes128CbcDec::new(key.into(), iv.into());
    dec.decrypt_padded_vec_mut::<Pkcs7>(body)
        .map_err(|e| format!("aes-cbc decrypt failed: {e}"))
}
