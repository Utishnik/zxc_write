pub mod aes256 {
    use aes_gcm::aead::{Aead, KeyInit, Result};
    use aes_gcm::{Aes256Gcm, Key, Nonce};
    use getrandom::fill;

    pub fn generate_key_aes256() -> core::result::Result<[u8; 32], getrandom::Error> {
        let mut buf = [0u8; 32];
        fill(&mut buf)?;
        Ok(buf)
    }

    pub fn generate_nonce() -> core::result::Result<[u8; 12], getrandom::Error> {
        let mut buf = [0u8; 12];
        fill(&mut buf)?;
        Ok(buf)
    }

    pub fn encrypt(key: &[u8; 32], data: &[u8], nonce_get: &[u8; 12]) -> Result<Vec<u8>> {
        let key = Key::<Aes256Gcm>::from_slice(key);
        let cipher = Aes256Gcm::new(key);
        let nonce = Nonce::from_slice(nonce_get);
        cipher.encrypt(nonce, data)
    }

    pub fn more_encrypt(
        key: &[u8; 32],
        data: &[&[u8]],
        nonce_get: &[u8; 12],
    ) -> Result<Vec<Result<Vec<u8>>>> {
        let key = Key::<Aes256Gcm>::from_slice(key);
        let cipher = Aes256Gcm::new(key);
        let nonce = Nonce::from_slice(nonce_get);
        let mut ret = Vec::with_capacity(data.len());
        for &item in data.iter() {
            let crypt = cipher.encrypt(nonce, item);
            ret.push(crypt);
        }
        Ok(ret)
    }

    pub fn decrypt(key: &[u8; 32], data: &[u8], nonce_get: &[u8; 12]) -> Result<Vec<u8>> {
        let key = Key::<Aes256Gcm>::from_slice(key);
        let nonce = Nonce::from_slice(nonce_get);
        let cipher = Aes256Gcm::new(key);
        cipher.decrypt(nonce, data)
    }

    #[test]
    fn t() {
        let msg = b"Hello World";
        let key = generate_key_aes256().unwrap();
        let n = generate_nonce().unwrap();
        let ec = encrypt(&key, msg, &n);
        let ec_str = String::from_utf8_lossy(ec.clone().unwrap().as_slice()).to_string();
        println!("{ec_str}");
        let dc = decrypt(&key, ec.unwrap().as_slice(), &n);
        let dc_str = String::from_utf8_lossy(dc.unwrap().as_slice()).to_string();
        println!("{dc_str}");
    }
}

pub mod aes128 {
    use aes_gcm::{
        Aes128Gcm, Nonce,
        aead::{Aead, Key, KeyInit, Result},
    };
    use getrandom;
    pub fn generate_key_aes128() -> core::result::Result<[u8; 16], getrandom::Error> {
        let mut buf = [0u8; 16];
        getrandom::fill(&mut buf)?;
        Ok(buf)
    }
    pub fn generate_nonce() -> core::result::Result<[u8; 12], getrandom::Error> {
        let mut buf = [0u8; 12];
        getrandom::fill(&mut buf)?;
        Ok(buf)
    }
    pub fn encrypt(key: &[u8; 16], data: &[u8], nonce_get: &[u8; 12]) -> Result<Vec<u8>> {
        let key = Key::<Aes128Gcm>::from_slice(key);
        let cipher = Aes128Gcm::new(key);
        let nonce = Nonce::from_slice(nonce_get);
        cipher.encrypt(nonce, data)
    }
    pub fn decrypt(key: &[u8; 16], data: &[u8], nonce_get: &[u8; 12]) -> Result<Vec<u8>> {
        let key = Key::<Aes128Gcm>::from_slice(key);
        let nonce = Nonce::from_slice(nonce_get);
        let cipher = Aes128Gcm::new(key);
        cipher.decrypt(nonce, data)
    }

    pub fn more_encrypt(
        key: &[u8; 32],
        data: &[&[u8]],
        nonce_get: &[u8; 12],
    ) -> Result<Vec<Result<Vec<u8>>>> {
        let key = Key::<Aes128Gcm>::from_slice(key);
        let cipher = Aes128Gcm::new(key);
        let nonce = Nonce::from_slice(nonce_get);
        let mut ret = Vec::with_capacity(data.len());
        for &item in data.iter() {
            let crypt = cipher.encrypt(nonce, item);
            ret.push(crypt);
        }
        Ok(ret)
    }

    #[test]
    fn t() {
        let msg = b"Hello World";
        let key = generate_key_aes128().unwrap();
        let n = generate_nonce().unwrap();
        let ec = encrypt(&key, msg, &n);
        let ec_str = String::from_utf8_lossy(ec.clone().unwrap().as_slice()).to_string();
        println!("{ec_str}");
        let dc = decrypt(&key, ec.unwrap().as_slice(), &n);
        let dc_str = String::from_utf8_lossy(dc.unwrap().as_slice()).to_string();
        println!("{dc_str}");
    }
}
