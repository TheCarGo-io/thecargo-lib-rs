use std::time::{SystemTime, UNIX_EPOCH};

use aes::Aes128;
use aes::cipher::block_padding::Pkcs7;
use aes::cipher::{BlockDecryptMut, BlockEncryptMut, KeyIvInit};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE;
use hmac::{Hmac, Mac};
use sha2::Sha256;

type Encryptor = cbc::Encryptor<Aes128>;
type Decryptor = cbc::Decryptor<Aes128>;
type HmacSha256 = Hmac<Sha256>;

const VERSION: u8 = 0x80;
const TIMESTAMP_LEN: usize = 8;
const IV_LEN: usize = 16;
const TAG_LEN: usize = 32;
const HEADER_LEN: usize = 1 + TIMESTAMP_LEN + IV_LEN;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum FernetError {
    #[error("{0} is not a urlsafe base64 Fernet key")]
    InvalidKey(String),
    #[error("{0} is empty")]
    MissingKey(String),
    #[error("no configured key opens the token")]
    InvalidToken,
}

#[derive(Clone)]
pub struct Fernet {
    signing: [u8; 16],
    encryption: [u8; 16],
}

impl std::fmt::Debug for Fernet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Fernet(..)")
    }
}

fn mac(signing: &[u8; 16]) -> HmacSha256 {
    #[expect(clippy::expect_used, reason = "HMAC accepts a key of any length")]
    HmacSha256::new_from_slice(signing).expect("hmac key")
}

impl Fernet {
    pub fn new(key: &str, name: &str) -> Result<Self, FernetError> {
        let raw = URL_SAFE.decode(key).map_err(|_error| FernetError::InvalidKey(name.to_owned()))?;
        let (signing, encryption): ([u8; 16], [u8; 16]) = match (raw.get(..16), raw.get(16..)) {
            (Some(signing), Some(encryption)) if raw.len() == 32 => (
                signing.try_into().map_err(|_error| FernetError::InvalidKey(name.to_owned()))?,
                encryption.try_into().map_err(|_error| FernetError::InvalidKey(name.to_owned()))?,
            ),
            _ => return Err(FernetError::InvalidKey(name.to_owned())),
        };
        Ok(Self { signing, encryption })
    }

    pub fn encrypt(&self, plaintext: &[u8]) -> String {
        let timestamp = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |elapsed| elapsed.as_secs());
        self.encrypt_at(plaintext, timestamp, rand::random())
    }

    pub fn encrypt_at(&self, plaintext: &[u8], timestamp: u64, iv: [u8; IV_LEN]) -> String {
        let ciphertext = Encryptor::new(&self.encryption.into(), &iv.into()).encrypt_padded_vec_mut::<Pkcs7>(plaintext);
        let mut token = Vec::with_capacity(HEADER_LEN + ciphertext.len() + TAG_LEN);
        token.push(VERSION);
        token.extend_from_slice(&timestamp.to_be_bytes());
        token.extend_from_slice(&iv);
        token.extend_from_slice(&ciphertext);
        let mut signer = mac(&self.signing);
        signer.update(&token);
        token.extend_from_slice(&signer.finalize().into_bytes());
        URL_SAFE.encode(token)
    }

    pub fn decrypt(&self, token: &str) -> Result<Vec<u8>, FernetError> {
        let data = URL_SAFE.decode(token).map_err(|_error| FernetError::InvalidToken)?;
        if data.len() < HEADER_LEN + TAG_LEN || data[0] != VERSION {
            return Err(FernetError::InvalidToken);
        }
        let (body, tag) = data.split_at(data.len() - TAG_LEN);
        let mut verifier = mac(&self.signing);
        verifier.update(body);
        verifier.verify_slice(tag).map_err(|_error| FernetError::InvalidToken)?;
        let iv: [u8; IV_LEN] =
            body[1 + TIMESTAMP_LEN..HEADER_LEN].try_into().map_err(|_error| FernetError::InvalidToken)?;
        Decryptor::new(&self.encryption.into(), &iv.into())
            .decrypt_padded_vec_mut::<Pkcs7>(&body[HEADER_LEN..])
            .map_err(|_error| FernetError::InvalidToken)
    }
}

#[derive(Debug, Clone)]
pub struct MultiFernet {
    keys: Vec<Fernet>,
}

impl MultiFernet {
    pub fn new(keys: Vec<Fernet>) -> Option<Self> {
        (!keys.is_empty()).then_some(Self { keys })
    }

    pub fn from_settings(
        current: &str,
        old_keys: &str,
        current_name: &str,
        old_name: &str,
    ) -> Result<Self, FernetError> {
        let current = current.trim();
        if current.is_empty() {
            return Err(FernetError::MissingKey(current_name.to_owned()));
        }
        let mut keys = vec![Fernet::new(current, current_name)?];
        for key in old_keys.split(',').map(str::trim).filter(|key| !key.is_empty() && *key != current) {
            keys.push(Fernet::new(key, old_name)?);
        }
        Ok(Self { keys })
    }

    pub fn encrypt(&self, plaintext: &[u8]) -> String {
        self.keys[0].encrypt(plaintext)
    }

    pub fn encrypt_str(&self, plaintext: &str) -> String {
        self.encrypt(plaintext.as_bytes())
    }

    pub fn decrypt(&self, token: &str) -> Result<Vec<u8>, FernetError> {
        self.keys.iter().find_map(|key| key.decrypt(token).ok()).ok_or(FernetError::InvalidToken)
    }

    pub fn decrypt_str(&self, token: &str) -> Result<String, FernetError> {
        String::from_utf8(self.decrypt(token)?).map_err(|_error| FernetError::InvalidToken)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "cw_0x689RpI-jtRR7oE8h_eQsKImvJapLeSbXpwF4e4=";
    const TOKEN: &str =
        "gAAAAABlU_EAAAECAwQFBgcICQoLDA0OD0ibfXl3kOHZz69KRdI1KstYd7OFiYi3XMJ07xwd9ZL9uy8j_o1BwTTXtKHgZeavPA==";

    fn fernet() -> Fernet {
        Fernet::new(KEY, "KEY").unwrap()
    }

    #[test]
    fn a_python_token_decrypts() {
        assert_eq!(fernet().decrypt(TOKEN).unwrap(), b"hello loadboard");
    }

    #[test]
    fn the_python_token_is_reproduced_from_its_parts() {
        let iv: [u8; 16] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];
        assert_eq!(fernet().encrypt_at(b"hello loadboard", 1_700_000_000, iv), TOKEN);
    }

    #[test]
    fn a_fresh_token_round_trips() {
        let token = fernet().encrypt(b"secret value");
        assert_eq!(fernet().decrypt(&token).unwrap(), b"secret value");
    }

    #[test]
    fn a_tampered_token_is_rejected() {
        let mut bytes = URL_SAFE.decode(TOKEN).unwrap();
        bytes[30] ^= 0x01;
        assert_eq!(fernet().decrypt(&URL_SAFE.encode(bytes)), Err(FernetError::InvalidToken));
        assert_eq!(fernet().decrypt("not a token"), Err(FernetError::InvalidToken));
    }

    #[test]
    fn old_keys_still_open_rows_and_new_rows_use_the_current_key() {
        let fresh = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
        let multi = MultiFernet::from_settings(fresh, &format!(" {KEY} , {fresh} "), "CUR", "OLD").unwrap();
        assert_eq!(multi.keys.len(), 2);
        assert_eq!(multi.decrypt_str(TOKEN).unwrap(), "hello loadboard");
        let token = multi.encrypt_str("new");
        assert!(fernet().decrypt(&token).is_err());
        assert_eq!(multi.decrypt_str(&token).unwrap(), "new");
    }

    #[test]
    fn bad_settings_are_named() {
        assert_eq!(
            MultiFernet::from_settings("  ", "", "CUR", "OLD").unwrap_err(),
            FernetError::MissingKey("CUR".into())
        );
        assert_eq!(Fernet::new("short", "CUR").unwrap_err(), FernetError::InvalidKey("CUR".into()));
        assert_eq!(
            MultiFernet::from_settings(KEY, "bad", "CUR", "OLD").unwrap_err(),
            FernetError::InvalidKey("OLD".into())
        );
    }
}
