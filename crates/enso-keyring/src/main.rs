//! `enso-keyring` — the desktop's keyring: the Secret Service that gh, git,
//! VS Code, Chromium, and anything built on libsecret keep their passwords
//! and tokens in. Sealed on disk; opened with your login password, checked
//! against the system's own record, never kept.

#![cfg_attr(test, allow(clippy::unwrap_used))]

mod asking;
mod crypto;
mod machine;
mod prompt;
mod service;
mod store;

use std::{
    fs, io,
    path::PathBuf,
    process::ExitCode,
    sync::{Arc, Mutex},
    thread,
};

use enso_core::Context;

const HELP: &str = "\
enso-keyring — the desktop's keyring

Usage: enso-keyring             Serve the keyring (the session bus starts it)
       enso-keyring enable      Be the session's keyring, started when needed
       enso-keyring disable     Stop being it
       enso-keyring status      Say whether it's set up, running, unlocked
       enso-keyring lock        Lock it now
       enso-keyring try MODE    Show the card as it would look (create,
                                unlock, lost); nothing is checked

Programs keep passwords and tokens here through the Secret Service — gh,
git (credential.helper = libsecret), VS Code, Chromium, and anything built
on libsecret. It's sealed on disk (~/.local/share/enso/keyring.toml, yours
alone) to this machine and your account — systemd's credentials, with the
TPM — and opened when your login password, checked against the system's
own record, says it's you. The password is never kept or used to seal, so
resetting it (as root, say) changes nothing.

Options:
  -h, --help       Print this help
  -V, --version    Print the version
";

fn main() -> ExitCode {
    // Started again by itself, the kernel calls it `exe`.
    enso_core::exe::restore_name();
    let mut args = std::env::args().skip(1);
    let first = args.next();
    let result = match first.as_deref() {
        None => serve(),
        Some("prompt") => {
            // What's typed stays here: no core dumps, no peeking.
            let _ = rustix::process::set_dumpable_behavior(
                rustix::process::DumpableBehavior::NotDumpable,
            );
            let (mode, asker) = prompt::read_request();
            card(mode, asker, Some(Box::new(io::stdout())))
        }
        Some("try") => match args.next().as_deref().and_then(asking::Mode::from_word) {
            Some(mode) => card(mode, "gh".to_owned(), None),
            None => Err("try what? create, unlock, or lost".to_owned()),
        },
        Some("enable") => enable(true),
        Some("disable") => enable(false),
        Some("status") => status(),
        Some("lock") => call("Lock"),
        Some("-h" | "--help") => {
            print!("{HELP}");
            Ok(())
        }
        Some("-V" | "--version") => {
            println!("enso-keyring {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some(other) => Err(format!(
            "unknown command `{other}`\nTry 'enso-keyring --help'."
        )),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(why) => {
            eprintln!("enso-keyring: {why}");
            ExitCode::FAILURE
        }
    }
}

fn card(mode: asking::Mode, asker: String, out: Option<Box<dyn io::Write>>) -> Result<(), String> {
    let ctx = Context::load();
    let appearance = ctx.config.appearance.clone();
    let preview = out.is_none();
    // The screen with the focus, and the bar's clock there to open out of.
    let (screen, perch) = enso_ui::emerge::Perch::find();
    let card = prompt::Card::new(ctx, mode, asker, out).on(screen, perch);
    let result = enso_ui::run(card, &appearance)
        .map_err(|err| err.to_string());
    if result.is_err() && !preview {
        println!("cancel");
    }
    result
}

fn serve() -> Result<(), String> {
    let keyring = store::Keyring::load(&store::path())
        .map_err(|err| format!("can't read the keyring: {err}"))?;
    let state: service::State = Arc::new(Mutex::new(service::Shared::new(keyring)));
    let connection = zbus::block_on(async {
        let connection = zbus::connection::Builder::session().build().await?;
        // Everything on the bus before the name is taken: the call that
        // started this arrives the moment it is, and must find its object.
        service::publish(&connection, &state).await?;
        connection
            .request_name_with_flags(service::NAME, zbus::fdo::RequestNameFlags::DO_NOT_QUEUE)
            .await?;
        zbus::Result::Ok(connection)
    })
    .map_err(|err| format!("can't be the keyring (is another running?): {err}"))?;
    let _keep = connection;
    loop {
        thread::park();
    }
}

/// Call one of the service's own methods: `Lock`.
fn call(method: &str) -> Result<(), String> {
    zbus::block_on(async {
        let connection = zbus::Connection::session().await?;
        let none: Vec<zbus::OwnedObjectPath> = Vec::new();
        connection
            .call_method(
                Some(service::NAME),
                service::ROOT,
                Some("org.freedesktop.Secret.Service"),
                method,
                &(none,),
            )
            .await
            .map(drop)
    })
    .map_err(|err| format!("the keyring isn't answering: {err}"))?;
    println!("Locked.");
    Ok(())
}

fn service_file() -> PathBuf {
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/share")
        });
    data.join(format!("dbus-1/services/{}.service", service::NAME))
}

/// Become (or stop being) what the session bus starts for the keyring.
fn enable(on: bool) -> Result<(), String> {
    let file = service_file();
    if on {
        if let Some(dir) = file.parent() {
            fs::create_dir_all(dir).map_err(|err| err.to_string())?;
        }
        let text = format!(
            "# Written by `enso-keyring enable`; `enso-keyring disable` takes it away.\n[D-BUS Service]\nName={}\nExec={}\n",
            service::NAME,
            enso_core::exe::installed().display()
        );
        fs::write(&file, text).map_err(|err| err.to_string())?;
    } else if let Err(err) = fs::remove_file(&file)
        && err.kind() != io::ErrorKind::NotFound
    {
        return Err(err.to_string());
    }
    // The bus reads its service files again.
    let _ = zbus::block_on(async {
        let connection = zbus::Connection::session().await?;
        connection
            .call_method(
                Some("org.freedesktop.DBus"),
                "/org/freedesktop/DBus",
                Some("org.freedesktop.DBus"),
                "ReloadConfig",
                &(),
            )
            .await
            .map(drop)
    });
    if on {
        println!("enso-keyring is the session's keyring now.");
        println!("  service  {}", file.display());
        println!("  Started the first time a program wants a secret.");
    } else {
        println!("enso-keyring is no longer the session's keyring.");
    }
    Ok(())
}

fn status() -> Result<(), String> {
    println!(
        "set up      {}",
        if service_file().exists() {
            "yes"
        } else {
            "no — `enso-keyring enable`"
        }
    );
    let kept = store::path();
    println!(
        "keyring     {}",
        if kept.exists() {
            kept.display().to_string()
        } else {
            "not made yet: the first program to keep a secret makes it".to_owned()
        }
    );
    let locked: Result<bool, zbus::Error> = zbus::block_on(async {
        let connection = zbus::Connection::session().await?;
        let reply = connection
            .call_method(
                Some(service::NAME),
                service::collection_path("login").as_str(),
                Some("org.freedesktop.DBus.Properties"),
                "Get",
                &("org.freedesktop.Secret.Collection", "Locked"),
            )
            .await?;
        let value: zbus::OwnedVariant = reply.body().deserialize()?;
        Ok(bool::try_from(value).unwrap_or(true))
    });
    println!(
        "running     {}",
        match locked {
            Ok(true) => "yes, locked",
            Ok(false) => "yes, unlocked",
            Err(_) => "no",
        }
    );
    Ok(())
}
