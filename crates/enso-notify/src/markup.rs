//! Notification body markup, reduced to the text it carries.
//!
//! The specification lets bodies use a little HTML — `<b>`, `<i>`, `<u>`,
//! `<a href>`, `<img>` — and escape `<`, `>`, `&` as entities. Toasts are
//! small and quiet, so the styling is dropped and the words kept.

/// The plain text of a markup body.
pub fn plain(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    let mut rest = body;
    while let Some(at) = rest.find(['<', '&']) {
        out.push_str(&rest[..at]);
        let tail = &rest[at..];
        let consumed = if tail.starts_with('<') {
            tag(tail, &mut out)
        } else {
            entity(tail, &mut out)
        };
        rest = &tail[consumed..];
    }
    out.push_str(rest);
    out.trim().to_owned()
}

/// Handle a `<` at the start of `text`; returns how many bytes it used.
fn tag(text: &str, out: &mut String) -> usize {
    let Some(end) = text.find('>') else {
        // Not a tag after all: "3 < 5".
        out.push('<');
        return 1;
    };
    let inner = text[1..end].trim();
    let name: String = inner
        .trim_start_matches('/')
        .chars()
        .take_while(char::is_ascii_alphanumeric)
        .collect::<String>()
        .to_ascii_lowercase();
    match name.as_str() {
        "br" | "p" | "div" if !out.ends_with('\n') && !out.is_empty() => out.push('\n'),
        "br" | "p" | "div" | "b" | "i" | "u" | "a" | "s" | "tt" | "span" | "big" | "small"
        | "sub" | "sup" | "strong" | "em" | "code" => {}
        "img" => {
            if let Some(alt) = attribute(inner, "alt") {
                out.push_str(&plain(alt));
            }
        }
        // Anything else was probably meant literally: "<none>", "a<b>c".
        _ => {
            out.push('<');
            return 1;
        }
    }
    end + 1
}

/// The value of `name="…"` inside a tag.
fn attribute<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let at = tag.to_ascii_lowercase().find(&format!("{name}="))?;
    let value = &tag[at + name.len() + 1..];
    let quote = value.chars().next().filter(|c| *c == '"' || *c == '\'')?;
    let value = &value[1..];
    value.find(quote).map(|end| &value[..end])
}

/// Handle a `&` at the start of `text`; returns how many bytes it used.
fn entity(text: &str, out: &mut String) -> usize {
    let decoded = text[1..].find(';').filter(|end| *end <= 8).and_then(|end| {
        let name = &text[1..=end];
        let c = match name {
            "amp" => '&',
            "lt" => '<',
            "gt" => '>',
            "quot" => '"',
            "apos" => '\'',
            "nbsp" => ' ',
            _ => {
                let code = name.strip_prefix('#')?;
                let value = match code.strip_prefix(['x', 'X']) {
                    Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                    None => code.parse().ok()?,
                };
                char::from_u32(value)?
            }
        };
        Some((c, end + 2))
    });
    match decoded {
        Some((c, used)) => {
            out.push(c);
            used
        }
        None => {
            out.push('&');
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::plain;

    #[test]
    fn styling_is_dropped_and_words_kept() {
        assert_eq!(
            plain("<b>Mira</b>: tea at <i>four</i>?"),
            "Mira: tea at four?"
        );
        assert_eq!(
            plain("See <a href=\"https://example.org/?a=1&amp;b=2\">the page</a>."),
            "See the page."
        );
        assert_eq!(plain("one<br>two<br/>three"), "one\ntwo\nthree");
        assert_eq!(plain("<p>first</p><p>second</p>"), "first\nsecond");
        assert_eq!(
            plain("<img src=\"x.png\" alt=\"a cat\"/> sent"),
            "a cat sent"
        );
        assert_eq!(plain("<IMG SRC='x.png'> sent"), "sent");
    }

    #[test]
    fn entities_are_decoded() {
        assert_eq!(plain("Tom &amp; Jerry &lt;3"), "Tom & Jerry <3");
        assert_eq!(plain("&quot;hi&quot; &apos;there&apos;"), "\"hi\" 'there'");
        assert_eq!(plain("caf&#233; &#x2615;"), "café ☕");
        // Decoded once, not twice.
        assert_eq!(plain("&amp;lt;"), "&lt;");
    }

    #[test]
    fn text_that_only_looks_like_markup_survives() {
        assert_eq!(plain("3 < 5 and 7 > 2"), "3 < 5 and 7 > 2");
        assert_eq!(plain("R&D, AT&T; fish & chips"), "R&D, AT&T; fish & chips");
        assert_eq!(plain("state: <none>"), "state: <none>");
        assert_eq!(plain("a <b"), "a <b");
        assert_eq!(plain("&#xZZ; &unknown;"), "&#xZZ; &unknown;");
        assert_eq!(plain(""), "");
        assert_eq!(plain("  plain  "), "plain");
    }
}
