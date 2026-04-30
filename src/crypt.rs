use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Key, Nonce};

fn get_random_nonce() -> Result<String, getrandom::Error> {
    let mut buf = [0u8; 16];
    getrandom::fill(&mut buf)?;
    Ok(unsafe { String::from_raw_parts(buf.as_mut_ptr(), buf.len(), buf.len()) })
}

pub fn encrypt(key: &[u8], data: &[u8], nonce_get: String) -> Vec<u8> {
    let key = Key::<Aes256Gcm>::from_slice(key);
    let cipher = Aes256Gcm::new(key);
    let nonce = Nonce::from_slice(nonce_get.as_bytes());
    cipher.encrypt(nonce, data.as_ref()).unwrap()
}

pub fn decrypt(key: &[u8], data: &[u8], nonce_get: String) -> Vec<u8> {
    let key = Key::<Aes256Gcm>::from_slice(key);
    let nonce = Nonce::from_slice(nonce_get.as_bytes());
    let cipher = Aes256Gcm::new(key);
    cipher.decrypt(nonce, data.as_ref()).unwrap()
}

#[test]
fn t() {
    let msg = "Hello World";
    let key = "123";
    let n = get_random_nonce().expect("RANDOM ERR");
    let ec = encrypt(key.as_bytes(), msg.as_bytes(), n.to_string());
    let ec_str = String::from_utf8_lossy(ec.as_slice()).to_string();
    println!("{ec_str}");
    let dc = decrypt(key.as_bytes(), ec_str.as_bytes(), n.to_string());
    let dc_str = String::from_utf8_lossy(dc.as_slice()).to_string();
    println!("{dc_str}");
}
