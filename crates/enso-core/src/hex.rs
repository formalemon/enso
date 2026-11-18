//! Bytes as hexadecimal text, and back.

/// `bytes` as lowercase hexadecimal, two digits each.
pub fn encode(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        text.push(char::from(DIGITS[usize::from(byte >> 4)]));
        text.push(char::from(DIGITS[usize::from(byte & 0xf)]));
    }
    text
}

/// The bytes `text` spells in hexadecimal (either case), if it does.
pub fn decode(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    text.as_bytes()
        .chunks(2)
        .map(|pair| Some(digit(pair[0])? << 4 | digit(pair[1])?))
        .collect()
}

/// One hexadecimal digit's value.
pub fn digit(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_ways() {
        assert_eq!(encode(&[0, 15, 16, 255]), "000f10ff");
        assert_eq!(decode("000f10FF").unwrap(), [0, 15, 16, 255]);
        assert!(decode("abc").is_none());
        assert!(decode("zz").is_none());
        assert_eq!(decode("").unwrap(), Vec::<u8>::new());
    }
}
