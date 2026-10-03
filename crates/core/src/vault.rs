//! Random-nonce, authenticated local secret-state storage using RustCrypto XChaCha20-Poly1305.
//! Do not use Olm's legacy pickle encryption with a reused key: it derives a fixed IV.
use anyhow::{Result, ensure};
use chacha20poly1305::{
    KeyInit, XChaCha20Poly1305, XNonce,
    aead::{Aead, Payload},
};
use serde::{Serialize, de::DeserializeOwned};
use zeroize::Zeroizing;

pub fn seal<T: Serialize>(value: &T, key: &[u8; 32], context: &[u8]) -> Result<String> {
    let plaintext = Zeroizing::new(serde_json::to_vec(value)?);
    let mut nonce_bytes = [0u8; 24];
    getrandom::fill(&mut nonce_bytes)?;
    let nonce = XNonce::from(nonce_bytes);
    let cipher = XChaCha20Poly1305::new_from_slice(key)?;
    let ciphertext = cipher.encrypt(
        &nonce,
        Payload {
            msg: &plaintext,
            aad: context,
        },
    )?;
    let mut encoded = nonce_bytes.to_vec();
    encoded.extend(ciphertext);
    Ok(format!("xc1:{}", hex::encode(encoded)))
}
pub fn unseal<T: DeserializeOwned>(encoded: &str, key: &[u8; 32], context: &[u8]) -> Result<T> {
    let bytes = hex::decode(
        encoded
            .strip_prefix("xc1:")
            .ok_or_else(|| anyhow::anyhow!("unsupported secret-state format"))?,
    )?;
    ensure!(bytes.len() >= 24 + 16, "truncated secret-state ciphertext");
    let nonce_bytes: [u8; 24] = bytes[..24].try_into()?;
    let nonce = XNonce::from(nonce_bytes);
    let cipher = XChaCha20Poly1305::new_from_slice(key)?;
    let plaintext = Zeroizing::new(cipher.decrypt(
        &nonce,
        Payload {
            msg: &bytes[24..],
            aad: context,
        },
    )?);
    Ok(serde_json::from_slice(&plaintext)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn random_nonces_and_record_binding() -> Result<()> {
        let key = [42; 32];
        let one = seal(&"secret state", &key, b"account")?;
        let two = seal(&"secret state", &key, b"account")?;
        assert_ne!(one, two);
        assert_ne!(&one[4..52], &two[4..52]);
        assert_eq!(unseal::<String>(&one, &key, b"account")?, "secret state");
        assert!(unseal::<String>(&one, &key, b"different session").is_err());
        assert!(unseal::<String>(&one, &[43; 32], b"account").is_err());
        let mut tampered = one;
        let last = tampered.pop().unwrap();
        tampered.push(if last == '0' { '1' } else { '0' });
        assert!(unseal::<String>(&tampered, &key, b"account").is_err());
        Ok(())
    }
}
