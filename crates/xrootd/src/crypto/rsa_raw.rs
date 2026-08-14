//! Raw RSA operations in the shape GSI expects.
//!
//! `XrdCryptosslRSA::EncryptPrivate` signs successive plaintext chunks of
//! `keysize - 11` bytes with PKCS#1 v1.5 padding and *no* DigestInfo prefix
//! (OpenSSL's `EVP_PKEY_sign` with `RSA_PKCS1_PADDING` over raw data);
//! `DecryptPublic` is the matching `verify_recover`. Neither is a signature
//! over a hash, so the ordinary signing APIs do not apply and the padded
//! modular exponentiation is done explicitly here.

use rsa::traits::{PrivateKeyParts, PublicKeyParts};
use rsa::{BigUint, RsaPrivateKey, RsaPublicKey};

/// Applies the private exponent to one padded block.
fn private_op(key: &RsaPrivateKey, block: &[u8]) -> Vec<u8> {
    let k = key.size();
    let m = BigUint::from_bytes_be(block);
    let c = m.modpow(key.d(), key.n());
    left_pad(&c.to_bytes_be(), k)
}

/// Applies the public exponent to one block.
fn public_op(key: &RsaPublicKey, block: &[u8]) -> Vec<u8> {
    let k = key.size();
    let c = BigUint::from_bytes_be(block);
    let m = c.modpow(key.e(), key.n());
    left_pad(&m.to_bytes_be(), k)
}

fn left_pad(v: &[u8], n: usize) -> Vec<u8> {
    if v.len() >= n {
        return v.to_vec();
    }
    let mut out = vec![0u8; n - v.len()];
    out.extend_from_slice(v);
    out
}

/// PKCS#1 v1.5 block type 1: `00 01 FF..FF 00 || M`.
fn pad_type1(msg: &[u8], k: usize) -> Result<Vec<u8>, String> {
    if msg.len() + 11 > k {
        return Err(format!("chunk of {} too long for {}-byte key", msg.len(), k));
    }
    let mut out = Vec::with_capacity(k);
    out.push(0x00);
    out.push(0x01);
    out.resize(k - msg.len() - 1, 0xff);
    out.push(0x00);
    out.extend_from_slice(msg);
    Ok(out)
}

fn unpad_type1(block: &[u8]) -> Result<Vec<u8>, String> {
    if block.len() < 11 || block[0] != 0x00 || block[1] != 0x01 {
        return Err("bad PKCS#1 type 1 header".into());
    }
    let sep = block[2..]
        .iter()
        .position(|&b| b != 0xff)
        .ok_or("no PKCS#1 separator")?
        + 2;
    if block[sep] != 0x00 || sep < 10 {
        return Err("malformed PKCS#1 padding".into());
    }
    Ok(block[sep + 1..].to_vec())
}

/// Signs `data` in `keysize - 11` byte chunks, concatenating the results.
pub fn encrypt_private(key: &RsaPrivateKey, data: &[u8]) -> Result<Vec<u8>, String> {
    let k = key.size();
    let chunk = k - 11;
    let mut out = Vec::new();
    for part in data.chunks(chunk) {
        let block = pad_type1(part, k)?;
        out.extend_from_slice(&private_op(key, &block));
    }
    Ok(out)
}

/// Recovers the plaintext from `keysize`-byte ciphertext blocks.
pub fn decrypt_public(key: &RsaPublicKey, data: &[u8]) -> Result<Vec<u8>, String> {
    let k = key.size();
    if data.len() % k != 0 {
        return Err(format!(
            "ciphertext of {} bytes is not a multiple of the {}-byte key size",
            data.len(),
            k
        ));
    }
    let mut out = Vec::new();
    for part in data.chunks(k) {
        let block = public_op(key, part);
        out.extend_from_slice(&unpad_type1(&block)?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsa::rand_core::OsRng;

    #[test]
    fn private_encrypt_round_trips_through_public_decrypt() {
        let sk = RsaPrivateKey::new(&mut OsRng, 1024).unwrap();
        let pk = RsaPublicKey::from(&sk);
        // Longer than one chunk, to exercise the block loop.
        let msg: Vec<u8> = (0..300u32).map(|i| (i % 251) as u8).collect();
        let sig = encrypt_private(&sk, &msg).unwrap();
        // 300 bytes split into ceil(300 / (128 - 11)) = 3 blocks.
        assert_eq!(sig.len(), 3 * sk.size());
        assert_eq!(decrypt_public(&pk, &sig).unwrap(), msg);
    }
}
