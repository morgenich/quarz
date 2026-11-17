use crate::format::CryptHeader;
use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use anyhow::{Result, anyhow, bail};
use argon2::{Algorithm, Argon2, Params, Version};
use rand::RngCore;

const CHECK_TEXT: &[u8] = b"quarz key check";
const NONCE_LEN: usize = 12;

#[derive(Clone)]
pub struct Key {
    cipher: Aes256Gcm,
}

fn stretch(password: &str, header: &CryptHeader) -> Result<Key> {
    let params = Params::new(header.mem_kib, header.passes, header.lanes, Some(32)).map_err(|e| anyhow!("Argon2: {e}"))?;
    let mut key = [0u8; 32];
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(password.as_bytes(), &header.salt, &mut key)
        .map_err(|e| anyhow!("Argon2: {e}"))?;
    let cipher = Aes256Gcm::new_from_slice(&key).map_err(|e| anyhow!("AES: {e}"))?;
    Ok(Key { cipher })
}

pub fn create(password: &str) -> Result<(CryptHeader, Key)> {
    let mut header = CryptHeader { salt: [0u8; 16], mem_kib: 64 * 1024, passes: 3, lanes: 1, check: Vec::new() };
    rand::thread_rng().fill_bytes(&mut header.salt);
    let key = stretch(password, &header)?;
    header.check = key.seal(CHECK_TEXT)?;
    Ok((header, key))
}

pub fn unlock(password: &str, header: &CryptHeader) -> Result<Key> {
    let key = stretch(password, header)?;
    match key.open(&header.check) {
        Ok(text) if text == CHECK_TEXT => Ok(key),
        _ => bail!("falsches Passwort"),
    }
}

impl Key {
    pub fn seal(&self, data: &[u8]) -> Result<Vec<u8>> {
        let mut nonce = [0u8; NONCE_LEN];
        rand::thread_rng().fill_bytes(&mut nonce);
        let sealed = self.cipher.encrypt(Nonce::from_slice(&nonce), data).map_err(|_| anyhow!("Verschluesselung fehlgeschlagen"))?;
        let mut out = Vec::with_capacity(NONCE_LEN + sealed.len());
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&sealed);
        Ok(out)
    }

    pub fn open(&self, data: &[u8]) -> Result<Vec<u8>> {
        if data.len() < NONCE_LEN + 16 {
            bail!("verschluesselte Daten sind zu kurz");
        }
        self.cipher
            .decrypt(Nonce::from_slice(&data[..NONCE_LEN]), &data[NONCE_LEN..])
            .map_err(|_| anyhow!("Daten beschaedigt oder falsches Passwort"))
    }
}
