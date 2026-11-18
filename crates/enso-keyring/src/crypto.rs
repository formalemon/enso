//! The cryptography, kept to three well-worn pieces from RustCrypto:
//!
//! - a **key from the password**: Argon2id, 64 MiB and three passes —
//!   slow and memory-hungry on purpose, so guessing is too;
//! - **authenticated encryption**: XChaCha20-Poly1305, whose 24-byte
//!   nonces are safe to choose at random, and which notices any change to
//!   what it sealed;
//! - **randomness** from the kernel (`getrandom`).
//!
//! The password itself is never kept: it unlocks the master key and is
//! wiped.

use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::{
    KeyInit, XChaCha20Poly1305, XNonce,
    aead::{Aead, Key},
};
use zeroize::Zeroizing;

/// A 256-bit key, wiped when dropped.
pub type Secret32 = Zeroizing<[u8; 32]>;

/// How hard the password is worked, kept with what it locked so later
/// changes to these don't strand an older keyring.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct Cost {
    /// Memory, in KiB.
    pub memory: u32,
    pub passes: u32,
}

impl Default for Cost {
    fn default() -> Self {
        Self {
            memory: 64 * 1024,
            passes: 3,
        }
    }
}

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

/// The key a password makes, with this salt and cost.
pub fn derive(password: &[u8], salt: &[u8; 16], cost: Cost) -> Result<Secret32, String> {
    let params =
        Params::new(cost.memory, cost.passes, 1, Some(32)).map_err(|err| err.to_string())?;
    let mut key = Zeroizing::new([0; 32]);
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(password, salt, key.as_mut())
        .map_err(|err| err.to_string())?;
    Ok(key)
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
    fn a_password_makes_the_same_key_each_time() {
        // Light, for the test's sake.
        let cost = Cost {
            memory: 1024,
            passes: 1,
        };
        let salt = [7; 16];
        let one = derive(b"login password", &salt, cost).unwrap();
        assert_eq!(one, derive(b"login password", &salt, cost).unwrap());
        assert_ne!(one, derive(b"login passwore", &salt, cost).unwrap());
        assert_ne!(one, derive(b"login password", &[8; 16], cost).unwrap());
        assert_eq!(unhex(&hex(&[0, 15, 255])).unwrap(), [0, 15, 255]);
    }
}
