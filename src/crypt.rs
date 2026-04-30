use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use getrandom::fill;

fn generate_key_aes256() -> Result<[u8; 32], getrandom::Error> {
    let mut buf = [0u8; 32];
    fill(&mut buf)?;
    Ok(buf)
}

fn generate_nonce() -> Result<[u8; 12], getrandom::Error> {
    let mut buf = [0u8; 12];
    fill(&mut buf)?;
    Ok(buf)
}

pub fn encrypt(key: &[u8; 32], data: &[u8], nonce_get: &[u8; 12]) -> Vec<u8> {
    let key = Key::<Aes256Gcm>::from_slice(key);
    let cipher = Aes256Gcm::new(key);
    let nonce = Nonce::from_slice(nonce_get);
    cipher.encrypt(nonce, data).unwrap()
}

pub fn decrypt(key: &[u8; 32], data: &[u8], nonce_get: &[u8; 12]) -> Vec<u8> {
    let key = Key::<Aes256Gcm>::from_slice(key);
    let nonce = Nonce::from_slice(nonce_get);
    let cipher = Aes256Gcm::new(key);
    cipher.decrypt(nonce, data).unwrap()
}

#[test]
fn t() {
    let msg = b"Hello World";
    let key = generate_key_aes256().unwrap();
    let n = generate_nonce().unwrap();
    let ec = encrypt(&key, msg, &n);
    let ec_str = String::from_utf8_lossy(ec.as_slice()).to_string();
    println!("{ec_str}");
    let dc = decrypt(&key, ec.as_slice(), &n);
    let dc_str = String::from_utf8_lossy(dc.as_slice()).to_string();
    println!("{dc_str}");
}
