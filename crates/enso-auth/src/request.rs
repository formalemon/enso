//! What the agent asks the prompt, and what comes back.
//!
//! The agent runs the prompt as a separate process for each question: it
//! writes the request to the prompt's standard input — its length in bytes
//! on a line, then that much TOML — and reads one line of answer from its
//! standard output. Afterwards the agent may write `close` on a line, which
//! puts the prompt away as if it had been answered "no".
//!
//! A password never comes back this way: the prompt hands it to polkit's
//! helper itself and only says whether that went well.

use std::io::{self, BufRead, Write};

use serde::{Deserialize, Serialize};

/// A question for whoever is at the screen.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Request {
    /// polkit wants a password: someone must be shown to be who they say,
    /// before something is allowed.
    Password {
        /// What is being allowed, in polkit's words ("Authentication is
        /// required to mount …").
        message: String,
        /// The polkit action, e.g. `org.freedesktop.udisks2.filesystem-mount`.
        action: String,
        /// The program asking, when polkit says.
        program: Option<String>,
        /// Who may answer, the person at the screen first.
        users: Vec<String>,
        /// polkit's cookie for this request: the helper needs it.
        cookie: String,
    },
    /// Bluetooth pairing by comparing numbers: does the device show this?
    Confirm { device: String, passkey: u32 },
    /// Bluetooth pairing with nothing to compare: pair at all?
    Authorize { device: String },
    /// A device not trusted yet wants to use a service (audio, input…).
    Service { device: String, service: String },
    /// Type the PIN the device wants (old devices: often `0000`).
    Pin { device: String },
    /// Type the number the device shows.
    Passkey { device: String },
    /// Type this on the device (a keyboard, usually).
    Show { device: String, code: String },
}

impl Request {
    /// Whether this is about Bluetooth.
    pub fn is_bluetooth(&self) -> bool {
        !matches!(self, Self::Password { .. })
    }
}

/// The answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Answer {
    /// Yes: allowed, paired, the password was right.
    Allow,
    /// No, or put away.
    Deny,
    Pin(String),
    Passkey(u32),
}

impl Answer {
    pub fn line(&self) -> String {
        match self {
            Self::Allow => "allow".to_owned(),
            Self::Deny => "deny".to_owned(),
            Self::Pin(pin) => format!("pin {pin}"),
            Self::Passkey(passkey) => format!("passkey {passkey}"),
        }
    }

    /// Read back a line; anything unexpected is a no.
    pub fn parse(line: &str) -> Self {
        let line = line.trim_end_matches('\n');
        match line.split_once(' ') {
            None if line == "allow" => Self::Allow,
            Some(("pin", pin)) if !pin.is_empty() => Self::Pin(pin.to_owned()),
            Some(("passkey", number)) => number.parse().map_or(Self::Deny, Self::Passkey),
            _ => Self::Deny,
        }
    }
}

/// Write `request` the way the prompt reads it.
pub fn write(request: &Request, mut to: impl Write) -> io::Result<()> {
    let text = toml::to_string(request).map_err(io::Error::other)?;
    writeln!(to, "{}", text.len())?;
    to.write_all(text.as_bytes())?;
    to.flush()
}

/// Read a request written by [`write`].
pub fn read(mut from: impl BufRead) -> io::Result<Request> {
    let mut line = String::new();
    from.read_line(&mut line)?;
    let len: usize = line
        .trim()
        .parse()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "no request length"))?;
    if len > 64 * 1024 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "request too long",
        ));
    }
    let mut text = vec![0; len];
    from.read_exact(&mut text)?;
    let text = String::from_utf8(text).map_err(io::Error::other)?;
    toml::from_str(&text).map_err(io::Error::other)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_and_answers_survive_the_trip() {
        let requests = [
            Request::Password {
                message: "Authentication is required\nto mount \"Windows\"".into(),
                action: "org.freedesktop.udisks2.filesystem-mount-system".into(),
                program: Some("/usr/bin/udisksctl".into()),
                users: vec!["ren".into(), "root".into()],
                cookie: "3-abc-1-def".into(),
            },
            Request::Confirm {
                device: "Headphones".into(),
                passkey: 4021,
            },
            Request::Show {
                device: "Keyboard".into(),
                code: "000123".into(),
            },
        ];
        let mut bytes = Vec::new();
        for request in &requests {
            write(request, &mut bytes).unwrap();
        }
        let mut from = io::BufReader::new(&bytes[..]);
        for request in &requests {
            assert_eq!(&read(&mut from).unwrap(), request);
        }
        for answer in [
            Answer::Allow,
            Answer::Deny,
            Answer::Pin("0000".into()),
            Answer::Passkey(12),
        ] {
            assert_eq!(Answer::parse(&answer.line()), answer);
        }
        assert_eq!(Answer::parse(""), Answer::Deny);
        assert_eq!(Answer::parse("passkey lots"), Answer::Deny);
    }
}
