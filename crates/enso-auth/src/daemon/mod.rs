//! The agent: one system-bus connection serving polkit's authentication
//! agent and BlueZ's pairing agent, registering with each, and registering
//! again whenever either service restarts.

mod bluez;
mod polkit;
mod prompts;

use std::{thread, time::Duration};

use zbus::{Connection, message::Header, names::BusName};

pub use prompts::Prompts;

/// How often to look whether polkit or BlueZ has restarted.
const LOOK_EVERY: Duration = Duration::from_secs(3);

pub fn run() -> Result<(), String> {
    let prompts = Prompts::start();
    let me = rustix::process::getuid().as_raw();
    let connection = zbus::block_on(
        zbus::connection::Builder::system()
            .serve_at(
                polkit::PATH,
                polkit::Agent {
                    prompts: prompts.clone(),
                    me,
                },
            )
            .serve_at(bluez::PATH, bluez::Agent { prompts })
            .build(),
    )
    .map_err(|err| format!("can't reach the system bus: {err}"))?;

    // The owner each was registered with: a new owner is a restart.
    let mut registered: [Option<String>; 2] = [None, None];
    loop {
        for (index, name) in [polkit::NAME, bluez::NAME].into_iter().enumerate() {
            let owner = zbus::block_on(owner(&connection, name));
            if owner.is_none() || owner == registered[index] {
                continue;
            }
            let result = zbus::block_on(async {
                if index == 0 {
                    polkit::register(&connection).await
                } else {
                    bluez::register(&connection).await
                }
            });
            match result {
                Ok(()) => {
                    eprintln!("enso-auth: answering for {name}");
                    registered[index] = owner;
                }
                Err(err) if index == 0 && registered[0].is_none() => {
                    // Without polkit there's nothing much to do; say why.
                    return Err(format!("polkit won't take this agent: {err}"));
                }
                Err(err) => {
                    eprintln!("enso-auth: {name} won't take this agent: {err}");
                    registered[index] = owner;
                }
            }
        }
        thread::sleep(LOOK_EVERY);
    }
}

async fn owner(connection: &Connection, name: &str) -> Option<String> {
    let reply = connection
        .call_method(
            Some(BusName::try_from("org.freedesktop.DBus").ok()?),
            "/org/freedesktop/DBus",
            Some("org.freedesktop.DBus"),
            "GetNameOwner",
            &(name,),
        )
        .await
        .ok()?;
    reply.body().deserialize::<String>().ok()
}

/// Whether a call came from the service named `name`: only polkit may ask
/// for a password, only BlueZ for a pairing code.
async fn sent_by(connection: &Connection, header: &Header<'_>, name: &str) -> bool {
    match (header.sender(), owner(connection, name).await) {
        (Some(sender), Some(owner)) => sender.as_str() == owner,
        _ => false,
    }
}
