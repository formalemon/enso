//! Checking a password the way polkit wants it checked: through its own
//! helper, which runs PAM as root and, if the password is right, tells
//! polkit itself. Nothing here can say "yes" to polkit; only the helper can.
//!
//! The helper is reached at `/run/polkit/agent-helper.socket` (polkit 124
//! and later start one per connection), or failing that run as the setuid
//! `polkit-agent-helper-1`. Either way the conversation is lines: the user's
//! name and polkit's cookie go in; PAM's questions come out
//! (`PAM_PROMPT_ECHO_OFF Password:`), each answered with a line; it ends in
//! `SUCCESS` or `FAILURE`. Lines from the helper are C-escaped.
//!
//! The helper is only reached once a password has been typed: a
//! conversation left unanswered counts as a failed attempt to PAM, and three
//! of those lock the account for a while (`pam_faillock`).

use std::{
    io::{self, BufRead, BufReader, Read, Write},
    os::unix::net::UnixStream,
    path::Path,
    process::{Command, Stdio},
};

use zeroize::Zeroizing;

const SOCKET: &str = "/run/polkit/agent-helper.socket";
const SETUID: [&str; 2] = [
    "/usr/lib/polkit-1/polkit-agent-helper-1",
    "/usr/libexec/polkit-agent-helper-1",
];

/// How a check ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// The password was right; polkit has been told.
    Accepted,
    /// It wasn't.
    Refused,
    /// A further question was put away unanswered.
    Abandoned,
}

/// Something PAM says along the way.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Said {
    /// A further question (a second factor, say); `echo` if what's typed
    /// may show.
    Ask {
        text: String,
        echo: bool,
    },
    Info(String),
    Error(String),
}

/// One line from the helper.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Line {
    Said(Said),
    Success,
    Failure,
}

/// Check `password` for `user`, answering polkit's request `cookie`.
/// `said` hears everything PAM says beyond the password question, and
/// answers further questions (`None` to give up).
pub fn check(
    user: &str,
    cookie: &str,
    password: Zeroizing<String>,
    said: impl FnMut(Said) -> Option<Zeroizing<String>>,
) -> io::Result<Verdict> {
    let (reader, writer) = open(user, cookie)?;
    converse(BufReader::new(reader), writer, password, said)
}

type Ends = (Box<dyn Read + Send>, Box<dyn Write + Send>);

fn open(user: &str, cookie: &str) -> io::Result<Ends> {
    if Path::new(SOCKET).exists() {
        match UnixStream::connect(SOCKET) {
            Ok(stream) => {
                let mut writer = stream.try_clone()?;
                writer.write_all(format!("{user}\n{cookie}\n").as_bytes())?;
                return Ok((Box::new(stream), Box::new(writer)));
            }
            Err(err) if !SETUID.iter().any(|path| Path::new(path).exists()) => return Err(err),
            Err(_) => {}
        }
    }
    let helper = SETUID
        .iter()
        .find(|path| Path::new(path).exists())
        .ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "polkit's helper isn't installed")
        })?;
    let mut child = Command::new(helper)
        .arg(user)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| io::Error::other("no stdin"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("no stdout"))?;
    stdin.write_all(format!("{cookie}\n").as_bytes())?;
    // The helper exits once the conversation ends; reap it then.
    std::thread::spawn(move || child.wait());
    Ok((Box::new(stdout), Box::new(stdin)))
}

/// The conversation, over any pair of ends (tests use a pretend helper).
fn converse(
    mut reader: impl BufRead,
    mut writer: impl Write,
    password: Zeroizing<String>,
    mut said: impl FnMut(Said) -> Option<Zeroizing<String>>,
) -> io::Result<Verdict> {
    let mut password = Some(password);
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            break Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "polkit's helper stopped without an answer",
            ));
        }
        let Some(line) = parse(&unescape(line.trim_end_matches('\n'))) else {
            break Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "polkit's helper said something unexpected",
            ));
        };
        let answer = match line {
            Line::Success => break Ok(Verdict::Accepted),
            Line::Failure => break Ok(Verdict::Refused),
            // The password question: the password typed already.
            Line::Said(Said::Ask { echo: false, .. }) if password.is_some() => password.take(),
            Line::Said(ask @ Said::Ask { .. }) => match said(ask) {
                Some(answer) => Some(answer),
                None => break Ok(Verdict::Abandoned),
            },
            Line::Said(other) => {
                said(other);
                None
            }
        };
        if let Some(answer) = answer {
            // A line break would end the answer early and start another.
            let sent = if answer.contains('\n') {
                Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "an answer can't have a line break in it",
                ))
            } else {
                writer
                    .write_all(answer.as_bytes())
                    .and_then(|()| writer.write_all(b"\n"))
                    .and_then(|()| writer.flush())
            };
            drop(answer);
            if let Err(err) = sent {
                break Err(err);
            }
        }
    }
}

fn parse(line: &str) -> Option<Line> {
    let said = |prefix: &str| line.strip_prefix(prefix).map(str::to_owned);
    if let Some(text) = said("PAM_PROMPT_ECHO_OFF ") {
        Some(Line::Said(Said::Ask { text, echo: false }))
    } else if let Some(text) = said("PAM_PROMPT_ECHO_ON ") {
        Some(Line::Said(Said::Ask { text, echo: true }))
    } else if let Some(text) = said("PAM_ERROR_MSG ") {
        Some(Line::Said(Said::Error(text)))
    } else if let Some(text) = said("PAM_TEXT_INFO ") {
        Some(Line::Said(Said::Info(text)))
    } else if line.starts_with("SUCCESS") {
        Some(Line::Success)
    } else if line.starts_with("FAILURE") {
        Some(Line::Failure)
    } else {
        None
    }
}

/// Undo C escapes (`\n`, `\\`, `\"`, `\303`…), as glib's `g_strcompress`.
fn unescape(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'\\' || i + 1 == bytes.len() {
            out.push(bytes[i]);
            i += 1;
            continue;
        }
        let next = bytes[i + 1];
        i += 2;
        out.push(match next {
            b'n' => b'\n',
            b't' => b'\t',
            b'r' => b'\r',
            b'b' => 8,
            b'f' => 12,
            b'v' => 11,
            b'0'..=b'7' => {
                let mut value = u32::from(next - b'0');
                for _ in 0..2 {
                    match bytes.get(i) {
                        Some(digit @ b'0'..=b'7') => {
                            value = value * 8 + u32::from(digit - b'0');
                            i += 1;
                        }
                        _ => break,
                    }
                }
                value as u8
            }
            other => other,
        });
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A pretend helper: what it says, and what it was told.
    fn run(
        lines: &str,
        password: &str,
        mut answers: Vec<Option<&str>>,
    ) -> (io::Result<Verdict>, String, Vec<Said>) {
        let mut sent = Vec::new();
        let mut heard = Vec::new();
        let password = Zeroizing::new(password.to_owned());
        let verdict = converse(lines.as_bytes(), &mut sent, password, |said| {
            heard.push(said.clone());
            match said {
                Said::Ask { .. } => answers
                    .remove(0)
                    .map(|answer| Zeroizing::new(answer.to_owned())),
                _ => None,
            }
        });
        (verdict, String::from_utf8(sent).unwrap(), heard)
    }

    #[test]
    fn the_password_answers_the_password_question() {
        let (verdict, sent, heard) = run(
            "PAM_PROMPT_ECHO_OFF Password: \nSUCCESS\n",
            "hunter2",
            vec![],
        );
        assert_eq!(verdict.unwrap(), Verdict::Accepted);
        assert_eq!(sent, "hunter2\n");
        assert!(heard.is_empty());

        let (verdict, ..) = run("PAM_PROMPT_ECHO_OFF Password: \nFAILURE\n", "wrong", vec![]);
        assert_eq!(verdict.unwrap(), Verdict::Refused);
    }

    #[test]
    fn further_questions_and_messages_are_passed_on() {
        let lines = "PAM_PROMPT_ECHO_OFF Password: \nPAM_TEXT_INFO Touch\\040the key\nPAM_PROMPT_ECHO_ON Code: \nSUCCESS\n";
        let (verdict, sent, heard) = run(lines, "pw", vec![Some("123456")]);
        assert_eq!(verdict.unwrap(), Verdict::Accepted);
        assert_eq!(sent, "pw\n123456\n");
        assert_eq!(heard[0], Said::Info("Touch the key".into()));
        assert_eq!(
            heard[1],
            Said::Ask {
                text: "Code: ".into(),
                echo: true
            }
        );

        let (verdict, ..) = run(
            "PAM_PROMPT_ECHO_OFF Password: \nPAM_PROMPT_ECHO_OFF Again: \n",
            "pw",
            vec![None],
        );
        assert_eq!(verdict.unwrap(), Verdict::Abandoned);
    }

    #[test]
    fn a_helper_that_stops_or_babbles_is_an_error_not_a_no() {
        assert!(
            run("PAM_PROMPT_ECHO_OFF Password: \n", "pw", vec![])
                .0
                .is_err()
        );
        assert!(run("hello\n", "pw", vec![]).0.is_err());
        let (verdict, sent, _) = run("PAM_PROMPT_ECHO_OFF Password: \nSUCCESS\n", "a\nb", vec![]);
        assert!(verdict.is_err());
        assert!(sent.is_empty(), "a line break never reaches the helper");
    }

    #[test]
    fn escapes_undone() {
        assert_eq!(unescape(r#"a\nb\\c\"d\303\251"#), "a\nb\\c\"d\u{e9}");
        assert_eq!(unescape(r#"trailing\"#), "trailing\\");
    }
}
