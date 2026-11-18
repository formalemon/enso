//! The cryptography, kept to two well-worn pieces:
//!
//! - **authenticated encryption**: XChaCha20-Poly1305 (RustCrypto), whose
//!   24-byte nonces are safe to choose at random, and which notices any
//!   change to what it sealed;
//! - **randomness** from the kernel (`getrandom`).

use chacha20poly1305::{
    KeyInit, XChaCha20Poly1305, XNonce,
    aead::{Aead, Key},
};
use zeroize::Zeroizing;

/// A 256-bit key, wiped when dropped.
pub type Secret32 = Zeroizing<[u8; 32]>;

/// Bytes from the kernel's random source.
pub fn random<const N: usize>() -> [u8; N] {
    let mut bytes = [0; N];
    let mut filled = 0;
    while filled < N {
        match rustix::rand::getrandom(&mut bytes[filled..], rustix::rand::GetRandomFlags::empty()) {
            Ok(n) => filled += n,
            Err(rustix::io::Errno::INTR) => {}
            // Without randomness nothing can be locked safely: stop.
            Err(err) => panic!("no randomness from the kernel: {err}"),
        }
    }
    bytes
}

/// `plain`, sealed with `key`: a fresh nonce, then the ciphertext.
pub fn seal(key: &[u8; 32], plain: &[u8]) -> Vec<u8> {
    let cipher = XChaCha20Poly1305::new(&Key::<XChaCha20Poly1305>::from(*key));
    let nonce = random::<24>();
    let sealed = cipher
        .encrypt(&XNonce::from(nonce), plain)
        // Only too much to encrypt at once fails; a keyring isn't that.
        .unwrap_or_default();
    [nonce.as_slice(), &sealed].concat()
}

/// What `seal` sealed, if `key` is the one and nothing changed it.
pub fn open(key: &[u8; 32], sealed: &[u8]) -> Option<Zeroizing<Vec<u8>>> {
    if sealed.len() < 24 {
        return None;
    }
    let (nonce, body) = sealed.split_at(24);
    let nonce: [u8; 24] = nonce.try_into().ok()?;
    let cipher = XChaCha20Poly1305::new(&Key::<XChaCha20Poly1305>::from(*key));
    cipher
        .decrypt(&XNonce::from(nonce), body)
        .ok()
        .map(Zeroizing::new)
}

/// Lower-case hexadecimal.
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub fn unhex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(text.get(at..at + 2)?, 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sealed_opens_only_with_its_key_and_untouched() {
        let key = Zeroizing::new(random::<32>());
        let sealed = seal(&key, b"hunter2");
        assert_eq!(open(&key, &sealed).unwrap().as_slice(), b"hunter2");
        assert_ne!(seal(&key, b"hunter2"), sealed, "a fresh nonce each time");
        assert!(open(&random::<32>(), &sealed).is_none(), "another key");
        let mut changed = sealed.clone();
        *changed.last_mut().unwrap() ^= 1;
        assert!(open(&key, &changed).is_none(), "tampered");
    }

    #[test]
    fn hex_both_ways() {
        assert_eq!(unhex(&hex(&[0, 15, 255])).unwrap(), [0, 15, 255]);
        assert!(unhex("abc").is_none());
    }
}
