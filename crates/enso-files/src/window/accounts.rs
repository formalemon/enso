//! Signing in to GitHub, in the window: Settings → GitHub. The first time,
//! it asks for your OAuth app's client ID; then it shows a short code and
//! opens GitHub's page to type it at, copying it as it does. Approved
//! there, the token goes into the keyring and the account's name into the
//! settings. Disconnecting takes the token out of the keyring again.

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use enso_ui::{Clip, Cx, widget::TextField};

use super::{Files, vcs::Vcs};
use crate::{
    github::{self, Waiting},
    jobs::Message,
};

/// How long a code is waited on before giving up (GitHub's last 15 min).
const PATIENCE: Duration = Duration::from_secs(15 * 60);

impl Files {
    pub(super) fn github_account(&self) -> Option<&str> {
        self.overrides.github_account.as_deref()
    }

    /// Settings → GitHub was chosen.
    pub(super) fn github_row(&mut self, cx: &mut Cx<'_>) {
        if self.github_account().is_some() {
            self.vcs = Some(Vcs::Disconnect);
        } else if self.config.github_client_id.trim().is_empty() {
            self.vcs = Some(Vcs::ClientId(TextField::default(), None));
        } else {
            self.start_sign_in(cx);
        }
    }

    /// The client ID was given: keep it, and sign in.
    pub(super) fn set_client_id(&mut self, id: String, cx: &mut Cx<'_>) {
        self.overrides.github_client_id = Some(id);
        self.save_overrides(cx);
        self.start_sign_in(cx);
    }

    /// Ask GitHub for a code, then wait for it to be approved, on a thread.
    pub(super) fn start_sign_in(&mut self, cx: &mut Cx<'_>) {
        let Some(proxy) = self.proxy.clone() else {
            return;
        };
        let client_id = self.config.github_client_id.trim().to_owned();
        let stop = Arc::new(AtomicBool::new(false));
        self.signing_in = Some(stop.clone());
        self.vcs = Some(Vcs::Device {
            code: String::new(),
            page: String::new(),
            error: None,
        });
        cx.redraw();
        let _ = thread::Builder::new()
            .name("enso-files-github".to_owned())
            .spawn(move || {
                let code = match github::begin(&client_id) {
                    Ok(code) => code,
                    Err(why) => {
                        proxy.send(Message::SignedIn(Err(why)));
                        return;
                    }
                };
                if !proxy.send(Message::Device(code.user_code.clone(), code.page.clone())) {
                    return;
                }
                let started = Instant::now();
                let mut interval = Duration::from_secs(code.interval.max(1));
                let result = loop {
                    thread::sleep(interval);
                    if stop.load(Ordering::Relaxed) {
                        return;
                    }
                    if started.elapsed() > PATIENCE {
                        break Err("The code ran out of time: sign in again".to_owned());
                    }
                    match github::poll(&client_id, &code.device_code) {
                        Waiting::Pending => {}
                        Waiting::SlowDown => interval += Duration::from_secs(5),
                        Waiting::Failed(why) => break Err(why),
                        Waiting::Token(token) => {
                            break github::login(&token).and_then(|login| {
                                github::keep(&login, &token)?;
                                Ok(login)
                            });
                        }
                    }
                };
                proxy.send(Message::SignedIn(result));
            });
    }

    pub(super) fn device_arrived(&mut self, code: String, page: String, cx: &mut Cx<'_>) {
        if let Some(Vcs::Device {
            code: shown,
            page: at,
            ..
        }) = &mut self.vcs
        {
            *shown = code;
            *at = page;
            cx.redraw();
        }
    }

    pub(super) fn signed_in(&mut self, result: Result<String, String>, cx: &mut Cx<'_>) {
        self.signing_in = None;
        match result {
            Ok(login) => {
                self.vcs = None;
                self.overrides.github_account = Some(login.clone());
                self.save_overrides(cx);
                self.toast(
                    format!("Connected to GitHub as {login} · the token is in your keyring"),
                    cx,
                );
            }
            Err(why) => match &mut self.vcs {
                Some(Vcs::Device { error, .. }) => *error = Some(why),
                _ => self.toast(why, cx),
            },
        }
        cx.redraw();
    }

    /// "Open GitHub": the page, in the browser, with the code copied to
    /// paste there.
    pub(super) fn open_device_page(&mut self, cx: &mut Cx<'_>) {
        let Some(Vcs::Device { code, page, .. }) = &self.vcs else {
            return;
        };
        if code.is_empty() {
            return;
        }
        cx.copy(Clip::text(code));
        let page = page.clone();
        if let Err(err) = enso_fs::open::uri(&page) {
            self.toast(format!("Can't open it ({err}): go to {page}"), cx);
        }
    }

    pub(super) fn cancel_sign_in(&mut self) {
        if let Some(stop) = self.signing_in.take() {
            stop.store(true, Ordering::Relaxed);
        }
        self.vcs = None;
    }

    pub(super) fn disconnect(&mut self, cx: &mut Cx<'_>) {
        self.vcs = None;
        let Some(login) = self.overrides.github_account.take() else {
            return;
        };
        let forgotten = github::forget(&login);
        if self.git_facts.github_git {
            let _ = github::use_for_git(false);
            self.git_facts.github_git = false;
        }
        self.save_overrides(cx);
        match forgotten {
            Ok(()) => self.toast(
                format!("Disconnected from GitHub · {login}'s token left the keyring"),
                cx,
            ),
            Err(why) => self.toast(
                format!("Disconnected, but the keyring kept the token: {why}"),
                cx,
            ),
        }
    }

    /// "git uses it" was switched.
    pub(super) fn toggle_github_git(&mut self, cx: &mut Cx<'_>) {
        let on = !self.git_facts.github_git;
        match github::use_for_git(on) {
            Ok(()) => {
                self.git_facts.github_git = on;
                self.toast(
                    if on {
                        "git pushes and pulls with your GitHub account now"
                    } else {
                        "git no longer asks enso-files for GitHub"
                    },
                    cx,
                );
            }
            Err(why) => self.toast(why, cx),
        }
    }
}
