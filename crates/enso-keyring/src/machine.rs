//! Sealing to this machine and this user: systemd's credential service
//! (`io.systemd.Credentials`, over varlink on its socket), scoped to the
//! user asking, with the key systemd picks — the host's key and the TPM
//! where there is one, bound to no PCRs, so firmware and Secure Boot
//! updates don't strand it; only clearing the TPM would. What it seals opens
//! again only here, only for you — whatever your password is now — and not
//! from a copy of the disk.
//!
//! Varlink is JSON, one message ending in a NUL each way; the two calls
//! here need nothing more than a few strings in and one out.

use std::{
    io::{self, BufRead, BufReader, Write},
    os::unix::net::UnixStream,
};

const SOCKET: &str = "/run/systemd/io.systemd.Credentials";
/// Sealed under this name; opening checks it.
const NAME: &str = "enso-keyring";

/// What seals the master key.
pub trait Sealer {
    fn seal(&self, plain: &[u8]) -> Result<String, String>;
    fn open(&self, sealed: &str) -> Result<Vec<u8>, String>;
}

/// systemd, on this machine.
#[derive(Debug, Default)]
pub struct Machine;

impl Sealer for Machine {
    fn seal(&self, plain: &[u8]) -> Result<String, String> {
        let request = format!(
            r#"{{"method":"io.systemd.Credentials.Encrypt","parameters":{{"name":"{NAME}","data":"{}","scope":"user","uid":{}}}}}"#,
            base64(plain),
            uid()
        );
        field(&call(&request)?, "blob").ok_or_else(|| "systemd gave nothing back".to_owned())
    }

    fn open(&self, sealed: &str) -> Result<Vec<u8>, String> {
        if !sealed
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '='))
        {
            return Err("the sealed key doesn't read".to_owned());
        }
        let request = format!(
            r#"{{"method":"io.systemd.Credentials.Decrypt","parameters":{{"name":"{NAME}","blob":"{sealed}","scope":"user","uid":{}}}}}"#,
            uid()
        );
        let data = field(&call(&request)?, "data")
            .ok_or_else(|| "systemd gave nothing back".to_owned())?;
        unbase64(&data).ok_or_else(|| "systemd's answer doesn't read".to_owned())
    }
}

fn uid() -> u32 {
    rustix::process::getuid().as_raw()
}

/// One varlink call: the reply, or the error systemd named.
fn call(request: &str) -> Result<String, String> {
    let talk = || -> io::Result<String> {
        let mut socket = UnixStream::connect(SOCKET)?;
        socket.write_all(request.as_bytes())?;
        socket.write_all(&[0])?;
        let mut reply = Vec::new();
        BufReader::new(socket).read_until(0, &mut reply)?;
        reply.pop();
        Ok(String::from_utf8_lossy(&reply).into_owned())
    };
    let reply = talk().map_err(|err| format!("systemd's credential service isn't there: {err}"))?;
    match field(&reply, "error") {
        Some(error) => Err(error.rsplit('.').next().unwrap_or(&error).to_owned()),
        None => Ok(reply),
    }
}

/// The string value of `"name":"…"` in a reply (none of these strings
/// holds a quote or an escape).
fn field(json: &str, name: &str) -> Option<String> {
    let key = format!("\"{name}\":\"");
    let start = json.find(&key)? + key.len();
    let len = json[start..].find('"')?;
    Some(json[start..start + len].to_owned())
}

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn base64(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        for (index, shift) in [18, 12, 6, 0].into_iter().enumerate() {
            if index <= chunk.len() {
                out.push(char::from(ALPHABET[(n >> shift) as usize & 63]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

pub fn unbase64(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    let mut bits = 0u32;
    let mut count = 0;
    for c in text.bytes().filter(|c| !c.is_ascii_whitespace()) {
        if c == b'=' {
            break;
        }
        let value = ALPHABET.iter().position(|&a| a == c)? as u32;
        bits = (bits << 6) | value;
        count += 6;
        if count >= 8 {
            count -= 8;
            out.push((bits >> count) as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_both_ways() {
        for bytes in [&b""[..], b"f", b"fo", b"foo", b"foob", &[0, 255, 7, 128, 9]] {
            assert_eq!(unbase64(&base64(bytes)).unwrap(), bytes);
        }
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(
            field(r#"{"parameters":{"blob":"abc="}}"#, "blob").as_deref(),
            Some("abc=")
        );
    }

    /// Sealed and opened by this machine's systemd and TPM.
    #[test]
    #[ignore = "uses this machine's TPM"]
    fn this_machine_seals_and_opens() {
        let sealed = Machine.seal(b"a master key").unwrap();
        assert_eq!(Machine.open(&sealed).unwrap(), b"a master key");
        assert!(Machine.open("AAAA").is_err());
    }
}
