//! Signing in to a forge, in the window: Settings → GitHub, or → GitLab.
//! The first time, it asks for your OAuth app's client ID (and, for
//! GitLab, where your GitLab is); then it shows a short code and opens the
//! forge's page to type it at, copying it as it does. Approved there, the
//! token goes into the keyring and the account's name into the settings.
//! Disconnecting takes the token out of the keyring again.

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use enso_ui::{Clip, Cx, widget::TextField};

use super::overlay::{Overlay, Reach};
use super::{
    Files,
    vcs::{AppForm, Vcs},
};
use crate::{
    forge::{Account, Forge, Site, Waiting},
    jobs::Message,
};

/// How long a code is waited on before giving up (GitHub's last 15 min).
const PATIENCE: Duration = Duration::from_secs(15 * 60);

impl Files {
    /// The account signed in to on `site`, by its login.
    pub(super) fn account(&self, site: Site) -> Option<&str> {
        match site {
            Site::GitHub => self.overrides.github_account.as_deref(),
            Site::GitLab => self.overrides.gitlab_account.as_deref(),
        }
    }

    /// `site`, where it is.
    pub(super) fn forge(&self, site: Site) -> Forge {
        match site {
            Site::GitHub => Forge::github(),
            Site::GitLab => Forge::gitlab(&self.config.gitlab_host),
        }
    }

    /// The client ID of your OAuth app on `site`.
    pub(super) fn client_id(&self, site: Site) -> String {
        match site {
            Site::GitHub => &self.config.github_client_id,
            Site::GitLab => &self.config.gitlab_client_id,
        }
        .trim()
        .to_owned()
    }

    /// The account on `site`, with all it takes to use it.
    pub(super) fn signed_in_to(&self, site: Site) -> Option<Account> {
        Some(Account {
            forge: self.forge(site),
            login: self.account(site)?.to_owned(),
            client_id: self.client_id(site),
        })
    }

    /// Every account signed in to.
    pub(super) fn accounts(&self) -> Vec<Account> {
        Site::ALL
            .into_iter()
            .filter_map(|site| self.signed_in_to(site))
            .collect()
    }

    fn set_account(&mut self, site: Site, login: Option<String>) {
        match site {
            Site::GitHub => self.overrides.github_account = login,
            Site::GitLab => self.overrides.gitlab_account = login,
        }
    }

    /// Settings → GitHub (or GitLab) was chosen.
    pub(super) fn account_row(&mut self, site: Site, cx: &mut Cx<'_>) {
        if self.account(site).is_some() {
            self.overlay = Some(Overlay::Vcs(Vcs::Disconnect(site)));
        } else if self.client_id(site).is_empty() {
            let host = match site {
                Site::GitHub => String::new(),
                Site::GitLab => self.forge(site).host,
            };
            self.overlay = Some(Overlay::Vcs(Vcs::ClientId(AppForm {
                site,
                id: TextField::default(),
                host: TextField::new(&host),
                focus: usize::from(site == Site::GitLab && host == "gitlab.com"),
                error: None,
            })));
        } else {
            self.start_sign_in(site, cx);
        }
    }

    /// The app was given: keep its ID (and where the forge is), and sign in.
    pub(super) fn set_client_id(
        &mut self,
        site: Site,
        id: String,
        host: Option<String>,
        cx: &mut Cx<'_>,
    ) {
        match site {
            Site::GitHub => self.overrides.github_client_id = Some(id),
            Site::GitLab => {
                self.overrides.gitlab_client_id = Some(id);
                if let Some(host) = host {
                    self.overrides.gitlab_host = Some(Forge::gitlab(&host).host);
                }
            }
        }
        self.save_overrides(cx);
        self.start_sign_in(site, cx);
    }

    /// Ask the forge for a code, then wait for it to be approved, on a
    /// thread.
    pub(super) fn start_sign_in(&mut self, site: Site, cx: &mut Cx<'_>) {
        let Some(proxy) = self.proxy.clone() else {
            return;
        };
        let forge = self.forge(site);
        let client_id = self.client_id(site);
        let stop = Arc::new(AtomicBool::new(false));
        self.signing_in = Some(stop.clone());
        self.overlay = Some(Overlay::Vcs(Vcs::Device {
            site,
            code: String::new(),
            page: String::new(),
            error: None,
        }));
        cx.redraw();
        let _ = thread::Builder::new()
            .name("enso-files-sign-in".to_owned())
            .spawn(move || {
                let code = match forge.begin(&client_id) {
                    Ok(code) => code,
                    Err(why) => {
                        proxy.send(Message::SignedIn(site, Err(why)));
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
                    match forge.poll(&client_id, &code.device_code) {
                        Waiting::Pending => {}
                        Waiting::SlowDown => interval += Duration::from_secs(5),
                        Waiting::Failed(why) => break Err(why),
                        Waiting::Token(tokens) => {
                            break forge.login(&tokens.access).and_then(|login| {
                                forge.keep(&login, &tokens)?;
                                Ok(login)
                            });
                        }
                    }
                };
                proxy.send(Message::SignedIn(site, result));
            });
    }

    pub(super) fn device_arrived(&mut self, code: String, page: String, cx: &mut Cx<'_>) {
        if let Some(Vcs::Device {
            code: shown,
            page: at,
            ..
        }) = self.overlay.vcs_mut()
        {
            *shown = code;
            *at = page;
            cx.redraw();
        }
    }

    pub(super) fn signed_in(
        &mut self,
        site: Site,
        result: Result<String, String>,
        cx: &mut Cx<'_>,
    ) {
        self.signing_in = None;
        match result {
            Ok(login) => {
                // The sign-in form, if it's still up; nothing else.
                self.overlay.take_vcs();
                self.set_account(site, Some(login.clone()));
                self.save_overrides(cx);
                self.toast(
                    format!(
                        "Connected to {} as {login} · the token is in your keyring",
                        self.forge(site).name()
                    ),
                    cx,
                );
            }
            Err(why) => match self.overlay.vcs_mut() {
                Some(Vcs::Device { error, .. }) => *error = Some(why),
                _ => self.toast(why, cx),
            },
        }
        cx.redraw();
    }

    /// "Open GitHub" (or GitLab): the page, in the browser, with the code
    /// copied to paste there.
    pub(super) fn open_device_page(&mut self, cx: &mut Cx<'_>) {
        let Some(Vcs::Device { code, page, .. }) = self.overlay.vcs() else {
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
        self.overlay.take_vcs();
    }

    pub(super) fn disconnect(&mut self, site: Site, cx: &mut Cx<'_>) {
        self.overlay.take_vcs();
        let Some(login) = self.account(site).map(str::to_owned) else {
            return;
        };
        self.set_account(site, None);
        let forge = self.forge(site);
        let forgotten = forge.forget(&login);
        if self.git_facts.helped(site) {
            let _ = forge.use_for_git(false);
            self.git_facts.set_helped(site, false);
        }
        self.save_overrides(cx);
        let name = forge.name();
        match forgotten {
            Ok(()) => self.toast(
                format!("Disconnected from {name} · {login}'s token left the keyring"),
                cx,
            ),
            Err(why) => self.toast(
                format!("Disconnected, but the keyring kept the token: {why}"),
                cx,
            ),
        }
    }

    /// "git pushes and pulls with it" was switched, for `site`.
    pub(super) fn toggle_forge_git(&mut self, site: Site, cx: &mut Cx<'_>) {
        let on = !self.git_facts.helped(site);
        let forge = self.forge(site);
        match forge.use_for_git(on) {
            Ok(()) => {
                self.git_facts.set_helped(site, on);
                let name = forge.name();
                self.toast(
                    if on {
                        format!("git pushes and pulls with your {name} account now")
                    } else {
                        format!("git no longer asks enso-files for {name}")
                    },
                    cx,
                );
            }
            Err(why) => self.toast(why, cx),
        }
    }
}
