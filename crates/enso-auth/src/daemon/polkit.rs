//! polkit's authentication agent (`org.freedesktop.PolicyKit1.AuthenticationAgent`).
//!
//! When a program asks polkit for something only an administrator may do —
//! mounting an internal disk, changing a system setting — polkit asks the
//! agent registered for the session to have someone prove who they are.
//! This one queues a password prompt; the prompt checks the password with
//! polkit's own helper, which tells polkit the outcome directly.

use std::collections::HashMap;

use zbus::{Connection, OwnedVariant, Variant, interface, message::Header, names::BusName};

use super::{Prompts, sent_by};
use crate::{
    request::{Answer, Request},
    users::{self, Identity},
};

pub const NAME: &str = "org.freedesktop.PolicyKit1";
const AUTHORITY: &str = "/org/freedesktop/PolicyKit1/Authority";
const AUTHORITY_INTERFACE: &str = "org.freedesktop.PolicyKit1.Authority";
pub const PATH: &str = "/dev/enso/PolkitAgent";

pub struct Agent {
    pub prompts: Prompts,
    /// The person at the screen.
    pub me: u32,
}

#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.freedesktop.PolicyKit1.Error")]
pub enum Error {
    #[zbus(error)]
    ZBus(zbus::Error),
    Failed(String),
    Cancelled(String),
}

#[interface(name = "org.freedesktop.PolicyKit1.AuthenticationAgent")]
impl Agent {
    #[allow(clippy::too_many_arguments)]
    async fn begin_authentication(
        &self,
        action_id: String,
        message: String,
        _icon_name: String,
        details: HashMap<String, String>,
        cookie: String,
        identities: Vec<(String, HashMap<String, OwnedVariant>)>,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
    ) -> Result<(), Error> {
        if !sent_by(connection, &header, NAME).await {
            return Err(Error::Failed("only polkit may ask".to_owned()));
        }
        let identities: Vec<Identity> = identities
            .iter()
            .filter_map(|(kind, fields)| match kind.as_str() {
                "unix-user" => fields
                    .get("uid")
                    .and_then(|v| u32::try_from(v).ok())
                    .map(Identity::User),
                "unix-group" => fields
                    .get("gid")
                    .and_then(|v| u32::try_from(v).ok())
                    .map(Identity::Group),
                _ => None,
            })
            .collect();
        let users = users::names(&identities, self.me);
        if users.is_empty() {
            return Err(Error::Failed("nobody here can answer".to_owned()));
        }
        // pkexec says which command; others, sometimes, which program.
        let program = details
            .get("command_line")
            .or_else(|| details.get("program"))
            .cloned();
        let request = Request::Password {
            message,
            action: action_id,
            program,
            users,
            cookie: cookie.clone(),
        };
        match self.prompts.ask(&cookie, request).await {
            Answer::Allow => Ok(()),
            _ => Err(Error::Cancelled("put away".to_owned())),
        }
    }

    async fn cancel_authentication(
        &self,
        cookie: String,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
    ) {
        if sent_by(connection, &header, NAME).await {
            self.prompts.cancel(&cookie);
        }
    }
}

/// Become the agent for this session.
pub async fn register(connection: &Connection) -> zbus::Result<()> {
    let session = session(connection).await;
    let mut subject: HashMap<&str, Variant<'_>> = HashMap::new();
    let kind = match &session {
        Some(id) => {
            subject.insert("session-id", Variant::from(id.as_str()));
            "unix-session"
        }
        // No session to name: this process, which polkit maps to one.
        None => {
            subject.insert("pid", Variant::from(std::process::id()));
            subject.insert("start-time", Variant::from(start_time()));
            "unix-process"
        }
    };
    let locale = std::env::var("LC_ALL")
        .or_else(|_| std::env::var("LANG"))
        .unwrap_or_else(|_| "C".to_owned());
    connection
        .call_method(
            Some(NAME),
            AUTHORITY,
            Some(AUTHORITY_INTERFACE),
            "RegisterAuthenticationAgent",
            // polkit takes the path as a plain string.
            &((kind, subject), locale.as_str(), PATH),
        )
        .await
        .map(drop)
}

/// The login session this runs in: as the session says, or as logind does
/// (the user's graphical session).
async fn session(connection: &Connection) -> Option<String> {
    if let Ok(id) = std::env::var("XDG_SESSION_ID")
        && !id.is_empty()
    {
        return Some(id);
    }
    let reply = connection
        .call_method(
            Some(BusName::try_from("org.freedesktop.login1").ok()?),
            "/org/freedesktop/login1/user/self",
            Some("org.freedesktop.DBus.Properties"),
            "Get",
            &("org.freedesktop.login1.User", "Display"),
        )
        .await
        .ok()?;
    let value: OwnedVariant = reply.body().deserialize().ok()?;
    let (id, _path): (String, zbus::OwnedObjectPath) = value.try_into().ok()?;
    (!id.is_empty()).then_some(id)
}

/// When this process started, in clock ticks since boot, as polkit wants.
fn start_time() -> u64 {
    std::fs::read_to_string("/proc/self/stat")
        .ok()
        .and_then(|stat| {
            // After the command name, which may hold spaces, in parentheses.
            let rest = stat.rsplit_once(')')?.1;
            rest.split_whitespace().nth(19)?.parse().ok()
        })
        .unwrap_or(0)
}
