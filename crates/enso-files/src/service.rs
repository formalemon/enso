//! `enso-files --service`: the desktop portal's file chooser.
//!
//! xdg-desktop-portal finds this service by its D-Bus name (see
//! `contrib/portal`) and hands it every application's request for a file:
//! `OpenFile`, `SaveFile`, `SaveFiles`, as the
//! `org.freedesktop.impl.portal.FileChooser` interface defines them. The
//! service never draws. For each request it runs `enso-files pick
//! --request` with the request as TOML on stdin, and turns the TOML answer
//! back into the portal's reply — so a crashing picker costs one dialog,
//! not every application's, and nothing stays in memory between them.
//!
//! While a picker is open, the request's handle is an object with a
//! `Close` method: the application (or the portal) calls it to take the
//! dialog away, and the picker is ended.

use std::{
    collections::HashMap,
    future::Future,
    io::{Read, Write},
    path::PathBuf,
    pin::Pin,
    process::{Command, ExitCode, Stdio},
    sync::{Arc, Mutex},
    task::{Context, Poll, Waker},
    thread,
};

use enso_fs::uri;
use zbus::{
    OwnedObjectPath, OwnedVariant, Variant, connection, interface, object_server::ObjectServer,
};

use crate::pick::{Answer, Choice, Filter, Mode, Pattern, Request};

/// The name xdg-desktop-portal looks for, from `enso.portal`.
pub const NAME: &str = "org.freedesktop.impl.portal.desktop.enso";
const PATH: &str = "/org/freedesktop/portal/desktop";

/// The portal's response codes.
const SUCCESS: u32 = 0;
const CANCELLED: u32 = 1;
const FAILED: u32 = 2;

type Options = HashMap<String, OwnedVariant>;
type Reply = (u32, HashMap<String, OwnedVariant>);

/// Claim the name and serve until killed. Another copy already serving
/// is not an error: it answers instead.
pub fn run() -> ExitCode {
    let served = zbus::block_on(async {
        let connection = connection::Builder::session()
            .name(NAME)
            .allow_name_replacements(false)
            .replace_existing_names(false)
            .serve_at(PATH, FileChooser)
            .build()
            .await?;
        // Serve for the life of the process.
        let _connection = connection;
        std::future::pending::<()>().await;
        Ok::<(), zbus::Error>(())
    });
    match served {
        Ok(()) => ExitCode::SUCCESS,
        Err(zbus::Error::NameTaken) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("enso-files: cannot serve the file chooser portal: {err}");
            ExitCode::FAILURE
        }
    }
}

struct FileChooser;

#[interface(name = "org.freedesktop.impl.portal.FileChooser")]
impl FileChooser {
    async fn open_file(
        &self,
        #[zbus(object_server)] server: &ObjectServer,
        handle: OwnedObjectPath,
        app_id: String,
        parent_window: String,
        title: String,
        options: Options,
    ) -> Reply {
        let mode = Mode::Open {
            multiple: get(&options, "multiple").unwrap_or(false),
            directory: get(&options, "directory").unwrap_or(false),
        };
        answer(
            server,
            handle,
            request(mode, &app_id, &parent_window, title, &options),
        )
        .await
    }

    async fn save_file(
        &self,
        #[zbus(object_server)] server: &ObjectServer,
        handle: OwnedObjectPath,
        app_id: String,
        parent_window: String,
        title: String,
        options: Options,
    ) -> Reply {
        let mode = Mode::Save {
            name: get::<String>(&options, "current_name").unwrap_or_default(),
        };
        let mut request = request(mode, &app_id, &parent_window, title, &options);
        request.current_file =
            get::<Vec<u8>>(&options, "current_file").map(|bytes| uri::from_portal_bytes(&bytes));
        if let (Mode::Save { name }, Some(file)) = (&mut request.mode, &request.current_file)
            && name.is_empty()
            && let Some(file_name) = file.file_name()
        {
            *name = file_name.to_string_lossy().into_owned();
        }
        answer(server, handle, request).await
    }

    async fn save_files(
        &self,
        #[zbus(object_server)] server: &ObjectServer,
        handle: OwnedObjectPath,
        app_id: String,
        parent_window: String,
        title: String,
        options: Options,
    ) -> Reply {
        let files: Vec<String> = get::<Vec<Vec<u8>>>(&options, "files")
            .unwrap_or_default()
            .iter()
            .map(|bytes| uri::from_portal_bytes(bytes).to_string_lossy().into_owned())
            .collect();
        let mode = Mode::SaveFiles { files };
        answer(
            server,
            handle,
            request(mode, &app_id, &parent_window, title, &options),
        )
        .await
    }

    #[zbus(property)]
    fn version(&self) -> u32 {
        4
    }
}

/// The request object at a dialog's handle, while it is open.
struct Pending {
    child: Arc<Mutex<Option<u32>>>,
}

#[interface(name = "org.freedesktop.impl.portal.Request")]
impl Pending {
    /// Take the dialog away.
    fn close(&self) {
        if let Some(pid) = self.child.lock().ok().and_then(|pid| *pid)
            && let Some(pid) = rustix::process::Pid::from_raw(pid as i32)
        {
            let _ = rustix::process::kill_process(pid, rustix::process::Signal::TERM);
        }
    }
}

/// What every request has in common, from the portal's options.
fn request(
    mode: Mode,
    app_id: &str,
    parent_window: &str,
    title: String,
    options: &Options,
) -> Request {
    let mut request = Request::new(mode);
    request.title = title;
    request.accept_label = get(options, "accept_label");
    request.filters = get::<Vec<PortalFilter>>(options, "filters")
        .unwrap_or_default()
        .into_iter()
        .map(filter_from)
        .collect();
    // The current filter is sent whole; it is one of the filters, or added.
    if let Some(current) = get::<PortalFilter>(options, "current_filter").map(filter_from) {
        let index = match request.filters.iter().position(|filter| *filter == current) {
            Some(index) => index,
            None => {
                request.filters.push(current);
                request.filters.len() - 1
            }
        };
        request.current_filter = Some(index);
    }
    request.choices = get::<Vec<PortalChoice>>(options, "choices")
        .unwrap_or_default()
        .into_iter()
        .map(choice_from)
        .collect();
    request.folder =
        get::<Vec<u8>>(options, "current_folder").map(|bytes| uri::from_portal_bytes(&bytes));
    request.parent = parent_window.strip_prefix("wayland:").map(str::to_owned);
    request.app_id = app_id.to_owned();
    request
}

/// `(name, [(0 for a glob or 1 for a MIME type, pattern)])`
type PortalFilter = (String, Vec<(u32, String)>);
/// `(id, label, [(option id, option label)], initial)`; no options makes
/// it a checkbox whose value is "true" or "false".
type PortalChoice = (String, String, Vec<(String, String)>, String);

fn filter_from((name, patterns): PortalFilter) -> Filter {
    Filter {
        name,
        patterns: patterns
            .into_iter()
            .map(|(kind, pattern)| match kind {
                1 => Pattern::Mime(pattern),
                _ => Pattern::Glob(pattern),
            })
            .collect(),
    }
}

fn filter_to(filter: &Filter) -> PortalFilter {
    (
        filter.name.clone(),
        filter
            .patterns
            .iter()
            .map(|pattern| match pattern {
                Pattern::Glob(glob) => (0, glob.clone()),
                Pattern::Mime(mime) => (1, mime.clone()),
            })
            .collect(),
    )
}

fn choice_from((id, label, options, initial): PortalChoice) -> Choice {
    if options.is_empty() {
        Choice::Toggle {
            id,
            label,
            on: initial == "true",
        }
    } else {
        let selected = if options.iter().any(|(option, _)| *option == initial) {
            initial
        } else {
            options[0].0.clone()
        };
        Choice::Options {
            id,
            label,
            options,
            selected,
        }
    }
}

/// An option's value, if present and of the type wanted.
fn get<T>(options: &Options, key: &str) -> Option<T>
where
    T: TryFrom<OwnedVariant>,
{
    let value = options.get(key)?.try_clone().ok()?;
    T::try_from(value).ok()
}

/// Ask a picker, and turn its answer into the portal's reply.
async fn answer(server: &ObjectServer, handle: OwnedObjectPath, request: Request) -> Reply {
    let child = Arc::new(Mutex::new(None));
    // Without the request object the dialog still works; only Close won't.
    let _ = server
        .at(
            &handle,
            Pending {
                child: child.clone(),
            },
        )
        .await;
    let (send, receive) = oneshot();
    let asked = request.clone();
    let spawned = thread::Builder::new()
        .name("enso-files-pick".to_owned())
        .spawn(move || send.send(pick(&asked, &child)));
    let outcome = match spawned {
        Ok(_) => receive.await,
        Err(err) => Err(format!("cannot start a picker: {err}")),
    };
    let _ = server.remove::<Pending, _>(&handle).await;
    match outcome {
        Ok(Some(answer)) => (SUCCESS, results(&request, answer)),
        Ok(None) => (CANCELLED, HashMap::new()),
        Err(why) => {
            eprintln!("enso-files: {why}");
            (FAILED, HashMap::new())
        }
    }
}

/// Run `enso-files pick --request` and read its answer: `None` if it was
/// cancelled (or closed).
fn pick(request: &Request, child_pid: &Mutex<Option<u32>>) -> Result<Option<Answer>, String> {
    let exe = picker_exe()?;
    let text =
        toml::to_string(request).map_err(|err| format!("cannot write the request: {err}"))?;
    let mut child = Command::new(exe)
        .args(["pick", "--request"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|err| format!("cannot start a picker: {err}"))?;
    if let Ok(mut pid) = child_pid.lock() {
        *pid = Some(child.id());
    }
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(text.as_bytes())
            .map_err(|err| format!("cannot hand the picker its request: {err}"))?;
    }
    let mut output = String::new();
    if let Some(mut stdout) = child.stdout.take() {
        let _ = stdout.read_to_string(&mut output);
    }
    let status = child
        .wait()
        .map_err(|err| format!("the picker went missing: {err}"))?;
    if let Ok(mut pid) = child_pid.lock() {
        *pid = None;
    }
    if !status.success() || output.trim().is_empty() {
        return Ok(None);
    }
    toml::from_str(&output)
        .map(Some)
        .map_err(|err| format!("the picker's answer doesn't read: {err}"))
}

/// The program to run pickers with: this one — or, when it has been
/// rebuilt or upgraded while the service ran, the new one at the same
/// path, which the kernel reports as the old path plus " (deleted)". If
/// nothing is there any more, the old copy still runs from memory.
fn picker_exe() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|err| format!("cannot find myself: {err}"))?;
    if exe.exists() {
        return Ok(exe);
    }
    let text = exe.to_string_lossy();
    if let Some(original) = text.strip_suffix(" (deleted)").map(PathBuf::from)
        && original.exists()
    {
        return Ok(original);
    }
    Ok(PathBuf::from("/proc/self/exe"))
}

/// The portal's results for `answer`.
fn results(request: &Request, answer: Answer) -> HashMap<String, OwnedVariant> {
    let paths: Vec<PathBuf> = match &request.mode {
        // The folder chosen, and in it every file to save.
        Mode::SaveFiles { files } => answer
            .paths
            .first()
            .map(|folder| files.iter().map(|name| folder.join(name)).collect())
            .unwrap_or_default(),
        _ => answer.paths,
    };
    let uris: Vec<String> = paths.iter().map(|path| uri::from_path(path)).collect();
    let mut results = HashMap::new();
    let mut put = |key: &str, value: Variant<'_>| {
        if let Ok(value) = OwnedVariant::try_from(value) {
            results.insert(key.to_owned(), value);
        }
    };
    put("uris", Variant::from(uris));
    if !answer.choices.is_empty() {
        put("choices", Variant::from(answer.choices));
    }
    if let Some(filter) = answer.filter.and_then(|index| request.filters.get(index)) {
        put("current_filter", Variant::from(filter_to(filter)));
    }
    results
}

/// A value from another thread, awaited without blocking the D-Bus thread
/// — so other requests, and `Close`, are answered while a picker is open.
fn oneshot<T>() -> (Sender<T>, Receiver<T>) {
    let shared = Arc::new(Mutex::new((None, None)));
    (Sender(shared.clone()), Receiver(shared))
}

type Slot<T> = Arc<Mutex<(Option<T>, Option<Waker>)>>;

struct Sender<T>(Slot<T>);
struct Receiver<T>(Slot<T>);

impl<T> Sender<T> {
    fn send(self, value: T) {
        if let Ok(mut slot) = self.0.lock() {
            slot.0 = Some(value);
            if let Some(waker) = slot.1.take() {
                waker.wake();
            }
        }
    }
}

impl<T> Future for Receiver<T> {
    type Output = T;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<T> {
        let Ok(mut slot) = self.0.lock() else {
            return Poll::Pending;
        };
        match slot.0.take() {
            Some(value) => Poll::Ready(value),
            None => {
                slot.1 = Some(cx.waker().clone());
                Poll::Pending
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(pairs: Vec<(&str, Variant<'_>)>) -> Options {
        pairs
            .into_iter()
            .map(|(key, value)| (key.to_owned(), OwnedVariant::try_from(value).unwrap()))
            .collect()
    }

    #[test]
    fn portal_options_become_a_request() {
        let filters: Vec<PortalFilter> = vec![
            (
                "Images".into(),
                vec![(0, "*.png".into()), (1, "image/jpeg".into())],
            ),
            ("Text".into(), vec![(1, "text/plain".into())]),
        ];
        let choices: Vec<PortalChoice> = vec![
            ("ro".into(), "Read-only".into(), vec![], "true".into()),
            (
                "enc".into(),
                "Encoding".into(),
                vec![
                    ("utf8".into(), "UTF-8".into()),
                    ("latin1".into(), "Latin-1".into()),
                ],
                "nonsense".into(),
            ),
        ];
        let options = options(vec![
            ("accept_label", Variant::from("_Upload")),
            ("filters", Variant::from(filters.clone())),
            ("current_filter", Variant::from(filters[1].clone())),
            ("choices", Variant::from(choices)),
            ("current_folder", Variant::from(b"/home/me\0".to_vec())),
        ]);
        let request = request(
            Mode::Open {
                multiple: true,
                directory: false,
            },
            "org.mozilla.firefox",
            "wayland:abc123",
            "Upload".into(),
            &options,
        );
        assert_eq!(request.accept_label.as_deref(), Some("_Upload"));
        assert_eq!(request.filters.len(), 2);
        assert_eq!(
            request.filters[0].patterns[1],
            Pattern::Mime("image/jpeg".into())
        );
        assert_eq!(request.current_filter, Some(1));
        assert_eq!(
            request.folder.as_deref(),
            Some(std::path::Path::new("/home/me"))
        );
        assert_eq!(request.parent.as_deref(), Some("abc123"));
        assert_eq!(
            request.choices[0],
            Choice::Toggle {
                id: "ro".into(),
                label: "Read-only".into(),
                on: true
            }
        );
        // An initial value that isn't an option falls back to the first.
        assert_eq!(request.choices[1].value(), "utf8");
    }

    #[test]
    fn answers_become_portal_results() {
        let mut request = Request::new(Mode::Open {
            multiple: true,
            directory: false,
        });
        request.filters = vec![filter_from(("Images".into(), vec![(0, "*.png".into())]))];
        let results = results(
            &request,
            Answer {
                paths: vec!["/home/me/a b.png".into(), "/tmp/c.png".into()],
                filter: Some(0),
                choices: vec![("ro".into(), "true".into())],
            },
        );
        let uris: Vec<String> = results["uris"].try_clone().unwrap().try_into().unwrap();
        assert_eq!(uris, ["file:///home/me/a%20b.png", "file:///tmp/c.png"]);
        let filter: PortalFilter = results["current_filter"]
            .try_clone()
            .unwrap()
            .try_into()
            .unwrap();
        assert_eq!(filter, ("Images".to_owned(), vec![(0, "*.png".to_owned())]));
        let choices: Vec<(String, String)> =
            results["choices"].try_clone().unwrap().try_into().unwrap();
        assert_eq!(choices, [("ro".to_owned(), "true".to_owned())]);
    }

    #[test]
    fn saving_many_files_puts_each_in_the_folder() {
        let request = Request::new(Mode::SaveFiles {
            files: vec!["one.txt".into(), "two.txt".into()],
        });
        let results = results(
            &request,
            Answer {
                paths: vec!["/home/me/out".into()],
                ..Answer::default()
            },
        );
        let uris: Vec<String> = results["uris"].try_clone().unwrap().try_into().unwrap();
        assert_eq!(
            uris,
            ["file:///home/me/out/one.txt", "file:///home/me/out/two.txt"]
        );
    }

    #[test]
    fn the_oneshot_delivers_across_threads() {
        let (send, receive) = oneshot();
        thread::spawn(move || send.send(42));
        assert_eq!(zbus::block_on(receive), 42);
    }
}
