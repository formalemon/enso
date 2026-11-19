//! A bar on every screen. `enso-bar` on its own draws nothing: it watches
//! the screens and keeps one `enso-bar --output NAME` on each — started as
//! a screen is plugged in, ended as it goes, started again if it falls over.
//! Each bar is the whole bar, its workspaces those of its own screen; what
//! it opens (the control center, the calendar) comes up on that screen too,
//! through `$ENSO_OUTPUT`.

use std::{
    collections::HashMap,
    env,
    process::Command,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use rustix::process::{Pid, Signal, kill_process};

/// What the watcher hears.
enum Heard {
    /// The screens there are now, by name.
    Screens(Vec<String>),
    /// A bar ended: its screen, and its process.
    Ended(String, u32),
    /// Time to start a bar that fell over again.
    Retry(String),
    /// The compositor went away.
    Lost(String),
}

/// One screen's bar.
struct Running {
    pid: u32,
    started: Instant,
}

/// A bar that ends sooner than this after starting has fallen over.
const SHORT_LIVED: Duration = Duration::from_secs(5);
/// How many times in a row one may, before it is left alone.
const TRIES: u32 = 3;

/// Keep a bar on each screen `wanted` names (every screen if it's empty)
/// until the compositor goes.
pub fn run(wanted: &[String]) -> Result<(), String> {
    let (tell, heard) = mpsc::channel::<Heard>();
    {
        let tell = tell.clone();
        thread::Builder::new()
            .name("enso-bar-screens".to_owned())
            .spawn(move || {
                let result = enso_ui::outputs::watch(|names| {
                    let _ = tell.send(Heard::Screens(names.to_vec()));
                });
                let err = result.err().map_or_else(|| "the compositor went away".to_owned(), |err| err.to_string());
                let _ = tell.send(Heard::Lost(err));
            })
            .map_err(|err| format!("cannot watch the screens: {err}"))?;
    }

    let mut screens: Vec<String> = Vec::new();
    let mut bars: HashMap<String, Running> = HashMap::new();
    let mut failures: HashMap<String, u32> = HashMap::new();
    for message in heard {
        match message {
            Heard::Screens(now) => {
                screens = now
                    .into_iter()
                    .filter(|name| wanted.is_empty() || wanted.contains(name))
                    .collect();
                // Gone: its bar goes too.
                bars.retain(|name, bar| {
                    let keep = screens.contains(name);
                    if !keep {
                        stop(bar.pid);
                    }
                    keep
                });
                for name in &screens {
                    if !bars.contains_key(name) {
                        failures.remove(name);
                        start(name, &tell, &mut bars);
                    }
                }
            }
            Heard::Ended(name, pid) => {
                let Some(bar) = bars.get(&name).filter(|bar| bar.pid == pid) else {
                    continue;
                };
                let short = bar.started.elapsed() < SHORT_LIVED;
                bars.remove(&name);
                if !screens.contains(&name) {
                    continue;
                }
                let tries = failures.entry(name.clone()).or_default();
                *tries = if short { *tries + 1 } else { 0 };
                if *tries >= TRIES {
                    eprintln!("enso-bar: the bar on {name} keeps ending; leaving that screen without one");
                    continue;
                }
                let tell = tell.clone();
                let wait = Duration::from_secs(u64::from(*tries).max(1));
                let _ = thread::Builder::new().name("enso-bar-retry".to_owned()).spawn(move || {
                    thread::sleep(wait);
                    let _ = tell.send(Heard::Retry(name));
                });
            }
            Heard::Retry(name) => {
                if screens.contains(&name) && !bars.contains_key(&name) {
                    start(&name, &tell, &mut bars);
                }
            }
            Heard::Lost(err) => {
                for bar in bars.values() {
                    stop(bar.pid);
                }
                return Err(err);
            }
        }
    }
    Ok(())
}

/// Start the bar for screen `name`, and listen for its end.
fn start(name: &str, tell: &mpsc::Sender<Heard>, bars: &mut HashMap<String, Running>) {
    let program = env::current_exe().unwrap_or_else(|_| "enso-bar".into());
    let spawned = Command::new(program)
        .arg("--output")
        .arg(name)
        .env("ENSO_OUTPUT", name)
        .spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(err) => {
            eprintln!("enso-bar: cannot start the bar on {name}: {err}");
            return;
        }
    };
    let pid = child.id();
    bars.insert(
        name.to_owned(),
        Running {
            pid,
            started: Instant::now(),
        },
    );
    let tell = tell.clone();
    let name = name.to_owned();
    let _ = thread::Builder::new()
        .name("enso-bar-child".to_owned())
        .spawn(move || {
            let _ = child.wait();
            let _ = tell.send(Heard::Ended(name, pid));
        });
}

fn stop(pid: u32) {
    if let Some(pid) = i32::try_from(pid).ok().and_then(Pid::from_raw) {
        let _ = kill_process(pid, Signal::TERM);
    }
}

/// In a bar the watcher started: end with it, rather than be left behind.
pub fn end_with_the_watcher() {
    let _ = rustix::process::set_parent_process_death_signal(Some(Signal::TERM));
    // Already gone before that was asked: the parent is no longer the one
    // that started us.
    if rustix::process::getppid().is_none_or(|pid| pid.as_raw_nonzero().get() == 1) {
        std::process::exit(0);
    }
}
