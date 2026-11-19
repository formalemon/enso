//! greetd's side of the conversation: its IPC, and the thread that carries
//! it on while the screen stays responsive.
//!
//! greetd listens on the socket named by `$GREETD_SOCK`. Each request is a
//! JSON object behind its length (a native-endian `u32`), and each is
//! answered in kind before the next is sent. greetd runs PAM, so every
//! prompt — the password, a one-time code, a warning that the account is
//! locked — comes back as an `auth_message` for the greeter to show.

use std::{
    fmt,
    io::{self, Read, Write},
    os::unix::net::UnixStream,
    path::Path,
    sync::mpsc,
    thread,
};

use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

/// Longest reply taken: greetd's are a sentence or two.
const MAX_REPLY: u32 = 1 << 20;
/// Room for a request, so that serialising one with a password in it
/// never has to grow (and so copy) the buffer: the longest password the
/// field takes, every character escaped, fits many times over.
const REQUEST_ROOM: usize = 16 * 1024;

/// What the greeter asks greetd.
#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request<'a> {
    /// Start logging `username` in: PAM's questions follow.
    CreateSession { username: &'a str },
    /// The answer to the last question; `None` for a message that only
    /// wanted acknowledging.
    PostAuthMessageResponse { response: Option<&'a str> },
    /// Logged in: run this once the greeter has gone.
    StartSession {
        cmd: &'a [String],
        env: &'a [String],
    },
    /// Forget the session being set up.
    CancelSession,
}

impl fmt::Debug for Request<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Never the answer: it is usually a password.
        match self {
            Self::CreateSession { username } => write!(f, "CreateSession({username})"),
            Self::PostAuthMessageResponse { response } => {
                write!(
                    f,
                    "PostAuthMessageResponse({})",
                    response.map_or("none", |_| "…")
                )
            }
            Self::StartSession { cmd, .. } => write!(f, "StartSession({cmd:?})"),
            Self::CancelSession => f.write_str("CancelSession"),
        }
    }
}

/// What greetd answers.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Response {
    Success,
    Error {
        error_type: ErrorKind,
        description: String,
    },
    AuthMessage {
        auth_message_type: Prompt,
        auth_message: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    /// The answers were wrong: not let in.
    AuthError,
    /// Anything else.
    Error,
}

/// The kinds of message PAM sends.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Prompt {
    /// A question whose answer may be shown as it is typed.
    Visible,
    /// A question whose answer is a secret.
    Secret,
    /// Something to read.
    Info,
    /// Something that went wrong, to read.
    Error,
}

/// `request` as it goes on the wire.
pub fn encode(request: &Request<'_>) -> io::Result<Zeroizing<Vec<u8>>> {
    let mut frame = Zeroizing::new(Vec::with_capacity(REQUEST_ROOM));
    frame.extend_from_slice(&[0; 4]);
    serde_json::to_writer(&mut *frame, request).map_err(io::Error::other)?;
    let length = u32::try_from(frame.len() - 4).map_err(io::Error::other)?;
    frame[..4].copy_from_slice(&length.to_ne_bytes());
    Ok(frame)
}

/// Read one length-prefixed message's body.
fn read_frame(reader: &mut impl Read) -> io::Result<Vec<u8>> {
    let mut length = [0; 4];
    reader.read_exact(&mut length)?;
    let length = u32::from_ne_bytes(length);
    if length > MAX_REPLY {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("a reply of {length} bytes"),
        ));
    }
    let mut body = vec![0; length as usize];
    reader.read_exact(&mut body)?;
    Ok(body)
}

/// Read one reply.
pub fn read_response(reader: &mut impl Read) -> io::Result<Response> {
    let body = read_frame(reader)?;
    serde_json::from_slice(&body).map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))
}

/// Something that answers requests the way greetd does.
pub trait Greetd: Send {
    fn ask(&mut self, request: &Request<'_>) -> io::Result<Response>;
}

/// greetd itself, or anything else speaking its protocol over a stream.
#[derive(Debug)]
pub struct Wire<S>(pub S);

impl Wire<UnixStream> {
    pub fn connect(path: &Path) -> io::Result<Self> {
        UnixStream::connect(path).map(Self)
    }
}

impl<S: Read + Write + Send> Greetd for Wire<S> {
    fn ask(&mut self, request: &Request<'_>) -> io::Result<Response> {
        self.0.write_all(&encode(request)?)?;
        self.0.flush()?;
        read_response(&mut self.0)
    }
}

/// The password [`Pretend`] takes.
pub const PRETEND_PASSWORD: &str = "enso";

/// A greetd that logs anyone in with [`PRETEND_PASSWORD`] and starts
/// nothing: for `enso-greet preview`, and for tests.
#[derive(Debug, Default)]
pub struct Pretend {
    /// Who is being logged in.
    user: Option<String>,
    /// The session would be started.
    pub started: Option<Vec<String>>,
}

impl Greetd for Pretend {
    fn ask(&mut self, request: &Request<'_>) -> io::Result<Response> {
        let error = |description: &str| Response::Error {
            error_type: ErrorKind::Error,
            description: description.to_owned(),
        };
        Ok(match request {
            Request::CreateSession { username } => {
                if self.user.is_some() {
                    return Ok(error("a session is already being created"));
                }
                self.user = Some((*username).to_owned());
                Response::AuthMessage {
                    auth_message_type: Prompt::Secret,
                    auth_message: "Password: ".to_owned(),
                }
            }
            Request::PostAuthMessageResponse { response } => match self.user {
                None => error("no session"),
                Some(_) if *response == Some(PRETEND_PASSWORD) => Response::Success,
                Some(_) => Response::Error {
                    error_type: ErrorKind::AuthError,
                    description: "pam_authenticate: AUTH_ERR".to_owned(),
                },
            },
            Request::StartSession { cmd, .. } => {
                self.started = Some(cmd.to_vec());
                Response::Success
            }
            Request::CancelSession => {
                self.user = None;
                Response::Success
            }
        })
    }
}

/// What the screen asks the conversation thread to do.
#[derive(Debug)]
pub enum Command {
    /// Start over logging `user` in, forgetting any session half set up.
    Begin(String),
    /// Answer the last question.
    Answer(Option<Zeroizing<String>>),
    /// Run this as the session.
    Start { cmd: Vec<String>, env: Vec<String> },
}

/// What the conversation thread tells the screen.
#[derive(Debug)]
pub enum Heard {
    Reply(Response),
    /// greetd can't be reached any more.
    Lost(String),
}

/// Carry out one command, as the conversation thread does.
pub fn carry(greetd: &mut dyn Greetd, command: Command) -> Heard {
    let reply = match &command {
        Command::Begin(user) => {
            // Whatever the answer — there may have been nothing to cancel.
            let _ = greetd.ask(&Request::CancelSession);
            greetd.ask(&Request::CreateSession { username: user })
        }
        Command::Answer(answer) => greetd.ask(&Request::PostAuthMessageResponse {
            response: answer.as_ref().map(|answer| answer.as_str()),
        }),
        Command::Start { cmd, env } => greetd.ask(&Request::StartSession { cmd, env }),
    };
    drop(command);
    match reply {
        Ok(reply) => Heard::Reply(reply),
        Err(err) => Heard::Lost(err.to_string()),
    }
}

/// Talk to `greetd` on a thread of its own: commands go in through the
/// returned sender, and each one's reply comes back through `tell`. The
/// thread ends when the sender is dropped or greetd is lost.
pub fn converse(
    mut greetd: Box<dyn Greetd>,
    tell: impl Fn(Heard) -> bool + Send + 'static,
) -> io::Result<mpsc::Sender<Command>> {
    let (sender, commands) = mpsc::channel::<Command>();
    thread::Builder::new()
        .name("enso-greetd".to_owned())
        .spawn(move || {
            for command in commands {
                let heard = carry(greetd.as_mut(), command);
                let lost = matches!(heard, Heard::Lost(_));
                if !tell(heard) || lost {
                    break;
                }
            }
        })?;
    Ok(sender)
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    fn frame(json: &str) -> Vec<u8> {
        let mut bytes = (json.len() as u32).to_ne_bytes().to_vec();
        bytes.extend_from_slice(json.as_bytes());
        bytes
    }

    fn body(request: &Request<'_>) -> serde_json::Value {
        let wire = encode(request).unwrap();
        let length = u32::from_ne_bytes(wire[..4].try_into().unwrap()) as usize;
        assert_eq!(length, wire.len() - 4);
        serde_json::from_slice(&wire[4..]).unwrap()
    }

    #[test]
    fn requests_are_greetds_json() {
        use serde_json::json;
        assert_eq!(
            body(&Request::CreateSession { username: "kai" }),
            json!({"type": "create_session", "username": "kai"})
        );
        assert_eq!(
            body(&Request::PostAuthMessageResponse {
                response: Some("pa\"ss")
            }),
            json!({"type": "post_auth_message_response", "response": "pa\"ss"})
        );
        assert_eq!(
            body(&Request::PostAuthMessageResponse { response: None }),
            json!({"type": "post_auth_message_response", "response": null})
        );
        let cmd = ["bash".to_owned(), "-l".to_owned()];
        let env = ["XDG_SESSION_TYPE=wayland".to_owned()];
        assert_eq!(
            body(&Request::StartSession {
                cmd: &cmd,
                env: &env
            }),
            json!({"type": "start_session", "cmd": ["bash", "-l"], "env": ["XDG_SESSION_TYPE=wayland"]})
        );
        assert_eq!(
            body(&Request::CancelSession),
            json!({"type": "cancel_session"})
        );
        // The password is never in the debug form.
        let shown = format!(
            "{:?}",
            Request::PostAuthMessageResponse {
                response: Some("hunter2")
            }
        );
        assert!(!shown.contains("hunter2"));
    }

    #[test]
    fn replies_as_greetd_sends_them() {
        let mut wire = Cursor::new(
            [
                frame(r#"{"type":"success"}"#),
                frame(
                    r#"{"type":"error","error_type":"auth_error","description":"pam_authenticate: AUTH_ERR"}"#,
                ),
                frame(r#"{"type":"error","error_type":"error","description":"no"}"#),
                frame(
                    r#"{"type":"auth_message","auth_message_type":"secret","auth_message":"Password: "}"#,
                ),
                frame(
                    r#"{"type":"auth_message","auth_message_type":"info","auth_message":"Hello"}"#,
                ),
                frame(r#"{"type":"surprise"}"#),
            ]
            .concat(),
        );
        assert_eq!(read_response(&mut wire).unwrap(), Response::Success);
        assert!(matches!(
            read_response(&mut wire).unwrap(),
            Response::Error {
                error_type: ErrorKind::AuthError,
                ..
            }
        ));
        assert!(matches!(
            read_response(&mut wire).unwrap(),
            Response::Error {
                error_type: ErrorKind::Error,
                ..
            }
        ));
        assert_eq!(
            read_response(&mut wire).unwrap(),
            Response::AuthMessage {
                auth_message_type: Prompt::Secret,
                auth_message: "Password: ".to_owned()
            }
        );
        assert!(matches!(
            read_response(&mut wire).unwrap(),
            Response::AuthMessage {
                auth_message_type: Prompt::Info,
                ..
            }
        ));
        assert!(read_response(&mut wire).is_err());
        // The stream ended.
        assert!(read_response(&mut wire).is_err());

        // A length past all reason is refused before anything is allocated.
        let mut huge = Cursor::new(u32::MAX.to_ne_bytes().to_vec());
        assert!(read_response(&mut huge).is_err());
    }

    /// A greetd on the far end of a socket, answering from a script.
    #[test]
    fn a_conversation_over_a_socket() {
        let (near, mut far) = UnixStream::pair().unwrap();
        let server = thread::spawn(move || {
            let mut seen = Vec::new();
            let replies = [
                r#"{"type":"success"}"#,
                r#"{"type":"auth_message","auth_message_type":"secret","auth_message":"Password: "}"#,
                r#"{"type":"success"}"#,
            ];
            for reply in replies {
                let request: serde_json::Value =
                    serde_json::from_slice(&read_frame(&mut far).unwrap()).unwrap();
                seen.push(request["type"].as_str().unwrap().to_owned());
                far.write_all(&frame(reply)).unwrap();
            }
            seen
        });
        let mut greetd = Wire(near);
        let Heard::Reply(reply) = carry(&mut greetd, Command::Begin("kai".to_owned())) else {
            panic!("lost");
        };
        assert!(matches!(reply, Response::AuthMessage { .. }));
        let answer = Command::Answer(Some(Zeroizing::new("pw".to_owned())));
        assert!(matches!(
            carry(&mut greetd, answer),
            Heard::Reply(Response::Success)
        ));
        assert_eq!(
            server.join().unwrap(),
            [
                "cancel_session",
                "create_session",
                "post_auth_message_response"
            ]
        );
        // The far end has gone: that is a lost greetd, not a reply.
        assert!(matches!(
            carry(&mut greetd, Command::Answer(None)),
            Heard::Lost(_)
        ));
    }
}
