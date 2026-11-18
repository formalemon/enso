//! Forges: GitHub, and GitLab — gitlab.com or one of your own. Signing in
//! with the device flow, and what an account allows: your repositories,
//! to clone; a new one, to publish a folder to; and being git's way in,
//! for push and pull.
//!
//! Signing in never passes a password through here: the forge shows a
//! page, you type the short code shown here, and approve; this asks, every
//! few seconds, whether you have, and is given a token. It needs an OAuth
//! app of your own on the forge — its client ID, a public identifier, is
//! all this knows of it. Tokens are kept in the session's keyring
//! (enso-keyring), never in a file. GitLab's last two hours, so its
//! refresh token is kept with it, and a fresh one is fetched as needed.

use std::{collections::HashMap, time::Duration};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use zeroize::Zeroizing;

const TIMEOUT: Duration = Duration::from_secs(20);
/// A token this close to running out is refreshed before it's used.
const EARLY: i64 = 120;

/// Which forge.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Site {
    GitHub,
    GitLab,
}

impl Site {
    pub const ALL: [Self; 2] = [Self::GitHub, Self::GitLab];

    pub fn name(self) -> &'static str {
        match self {
            Self::GitHub => "GitHub",
            Self::GitLab => "GitLab",
        }
    }
}

/// A forge, where it is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Forge {
    pub site: Site,
    /// `github.com`, `gitlab.com`, or a GitLab of your own.
    pub host: String,
}

/// What to show while signing in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceCode {
    pub device_code: String,
    /// What you type at the forge: `ABCD-1234`.
    pub user_code: String,
    pub page: String,
    /// Seconds between asking whether it's done.
    pub interval: u64,
}

/// An account's way in: its token, and for GitLab the token that gets a
/// new one, and when this one runs out (seconds since 1970).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tokens {
    pub access: Zeroizing<String>,
    pub refresh: Option<Zeroizing<String>>,
    pub expires: Option<i64>,
}

impl Tokens {
    /// As the keyring keeps them: one a line.
    fn kept(&self) -> Zeroizing<String> {
        let mut text = Zeroizing::new(String::with_capacity(512));
        text.push_str(&self.access);
        if let Some(refresh) = &self.refresh {
            text.push('\n');
            text.push_str(refresh);
            text.push('\n');
            text.push_str(&self.expires.unwrap_or(0).to_string());
        }
        text
    }

    /// From the keyring's lines. A bare token (as GitHub's are) is the
    /// access token alone.
    fn from_kept(text: &str) -> Option<Self> {
        let mut lines = text.lines();
        let access = lines.next().filter(|line| !line.is_empty())?;
        let refresh = lines.next().filter(|line| !line.is_empty());
        let expires = lines.next().and_then(|line| line.parse().ok());
        Some(Self {
            access: Zeroizing::new(access.to_owned()),
            refresh: refresh.map(|refresh| Zeroizing::new(refresh.to_owned())),
            expires,
        })
    }

    fn from_answer(value: &Value) -> Option<Self> {
        let access = value["access_token"].as_str()?;
        let created = value["created_at"]
            .as_i64()
            .unwrap_or_else(|| jiff::Timestamp::now().as_second());
        Some(Self {
            access: Zeroizing::new(access.to_owned()),
            refresh: value["refresh_token"]
                .as_str()
                .map(|refresh| Zeroizing::new(refresh.to_owned())),
            expires: value["expires_in"].as_i64().map(|lasts| created + lasts),
        })
    }
}

/// How signing in stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Waiting {
    /// Not approved yet.
    Pending,
    /// Asked too often: wait this many seconds more each time.
    SlowDown,
    Token(Tokens),
    /// Declined, expired, or wrong: why.
    Failed(String),
}

/// A repository of yours (or one you work on).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Repo {
    /// `owner/name`, or a GitLab project's whole path.
    pub name: String,
    pub clone_url: String,
    pub private: bool,
    pub description: String,
    /// The forge it's on, by name.
    pub forge: &'static str,
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(TIMEOUT))
        .user_agent("enso-files")
        // The forges' errors come with words worth showing.
        .http_status_as_error(false)
        .build()
        .into()
}

impl Forge {
    pub fn github() -> Self {
        Self {
            site: Site::GitHub,
            host: "github.com".to_owned(),
        }
    }

    /// GitLab at `host`: `gitlab.com` if it's empty; a scheme or a
    /// trailing slash typed with it are let go.
    pub fn gitlab(host: &str) -> Self {
        let host = host.trim();
        let host = host
            .strip_prefix("https://")
            .or_else(|| host.strip_prefix("http://"))
            .unwrap_or(host)
            .trim_end_matches('/');
        Self {
            site: Site::GitLab,
            host: if host.is_empty() {
                "gitlab.com".to_owned()
            } else {
                host.to_owned()
            },
        }
    }

    /// What to call it: "GitHub", "GitLab", or "GitLab (git.example.org)".
    pub fn name(&self) -> String {
        match self.site {
            Site::GitLab if self.host != "gitlab.com" => format!("GitLab ({})", self.host),
            site => site.name().to_owned(),
        }
    }

    fn web(&self) -> String {
        format!("https://{}", self.host)
    }

    fn api(&self) -> String {
        match self.site {
            Site::GitHub => "https://api.github.com".to_owned(),
            Site::GitLab => format!("https://{}/api/v4", self.host),
        }
    }

    /// Where to make the OAuth app this signs in with, and how.
    pub fn app_help(&self) -> String {
        match self.site {
            Site::GitHub => "Its client ID: make one at github.com/settings/developers → New OAuth App, with “Enable Device Flow” ticked (any address will do for the rest). It's public, not a secret.".to_owned(),
            Site::GitLab => format!(
                "Its application ID: make one at {}/-/user_settings/applications, with the api scope and “Confidential” unticked (any redirect URI will do). It's public, not a secret.",
                self.host
            ),
        }
    }

    fn json(
        &self,
        response: Result<ureq::http::Response<ureq::Body>, ureq::Error>,
    ) -> Result<Value, String> {
        let name = self.name();
        let mut response = response.map_err(|err| format!("{name} can't be reached: {err}"))?;
        let status = response.status();
        let value: Value = response
            .body_mut()
            .read_json()
            .map_err(|err| format!("{name}'s answer doesn't read: {err}"))?;
        if status.is_client_error() || status.is_server_error() {
            let message = value["message"]
                .as_str()
                .or(value["error_description"].as_str())
                .or(value["error"].as_str())
                .map_or_else(|| format!("it said no ({status})"), str::to_owned);
            return Err(format!("{name}: {message}"));
        }
        Ok(value)
    }

    /// Begin signing in with the OAuth app `client_id`.
    pub fn begin(&self, client_id: &str) -> Result<DeviceCode, String> {
        let (url, scope) = match self.site {
            Site::GitHub => (
                "https://github.com/login/device/code".to_owned(),
                "repo read:user",
            ),
            Site::GitLab => (format!("{}/oauth/authorize_device", self.web()), "api"),
        };
        let value = self
            .json(
                agent()
                    .post(&url)
                    .header("Accept", "application/json")
                    .send_form([("client_id", client_id), ("scope", scope)]),
            )
            .map_err(|why| {
                // GitHub says "Not Found"; GitLab, "unknown client".
                if ["404", "Not Found", "unknown client", "invalid_client"]
                    .iter()
                    .any(|said| why.contains(said))
                {
                    format!(
                        "{} doesn't know that client ID: check it on your OAuth app's page",
                        self.name()
                    )
                } else {
                    why
                }
            })?;
        if let Some(error) = value["error_description"]
            .as_str()
            .or(value["error"].as_str())
        {
            return Err(format!("{}: {error}", self.name()));
        }
        let missing = || format!("{} sent no code", self.name());
        Ok(DeviceCode {
            device_code: value["device_code"]
                .as_str()
                .ok_or_else(missing)?
                .to_owned(),
            user_code: value["user_code"].as_str().ok_or_else(missing)?.to_owned(),
            page: value["verification_uri"]
                .as_str()
                .map_or_else(|| format!("{}/login/device", self.web()), str::to_owned),
            interval: value["interval"].as_u64().unwrap_or(5),
        })
    }

    fn token_url(&self) -> String {
        match self.site {
            Site::GitHub => "https://github.com/login/oauth/access_token".to_owned(),
            Site::GitLab => format!("{}/oauth/token", self.web()),
        }
    }

    /// Ask whether signing in has been approved.
    pub fn poll(&self, client_id: &str, device_code: &str) -> Waiting {
        // GitHub says "not yet" with a 200, GitLab with a 400: the words
        // are what count, so they're read whatever the status.
        let answer = agent()
            .post(&self.token_url())
            .header("Accept", "application/json")
            .send_form([
                ("client_id", client_id),
                ("device_code", device_code),
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ]);
        let Some(value) = answer
            .ok()
            .and_then(|mut response| response.body_mut().read_json::<Value>().ok())
        else {
            // Offline for a moment: ask again.
            return Waiting::Pending;
        };
        if let Some(tokens) = Tokens::from_answer(&value) {
            return Waiting::Token(tokens);
        }
        match value["error"].as_str() {
            Some("authorization_pending") => Waiting::Pending,
            Some("slow_down") => Waiting::SlowDown,
            Some("access_denied") => Waiting::Failed(format!("It was declined at {}", self.name())),
            Some("expired_token") => {
                Waiting::Failed("The code ran out of time: sign in again".to_owned())
            }
            Some("device_flow_disabled") => Waiting::Failed(
                "The OAuth app needs “Enable Device Flow” ticked, on GitHub".to_owned(),
            ),
            Some(other) => Waiting::Failed(format!(
                "{}: {}",
                self.name(),
                value["error_description"].as_str().unwrap_or(other)
            )),
            None => Waiting::Pending,
        }
    }

    /// A fresh token for an expiring one, with the app `client_id`.
    fn refresh(&self, client_id: &str, refresh: &str) -> Result<Tokens, String> {
        let value = self.json(
            agent()
                .post(&self.token_url())
                .header("Accept", "application/json")
                .send_form([
                    ("client_id", client_id),
                    ("refresh_token", refresh),
                    ("grant_type", "refresh_token"),
                ]),
        )?;
        Tokens::from_answer(&value).ok_or_else(|| format!("{} sent no new token", self.name()))
    }

    fn get(&self, token: &str, url: &str) -> Result<Value, String> {
        self.json(
            agent()
                .get(url)
                .header("Authorization", bearer(token).as_str())
                .header("Accept", "application/json")
                .call(),
        )
    }

    /// The account a token is for: its login.
    pub fn login(&self, token: &str) -> Result<String, String> {
        let value = self.get(token, &format!("{}/user", self.api()))?;
        let field = match self.site {
            Site::GitHub => "login",
            Site::GitLab => "username",
        };
        value[field]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| format!("{} didn't say who you are", self.name()))
    }

    fn repo(&self, value: &Value) -> Option<Repo> {
        let (name, url, private) = match self.site {
            Site::GitHub => (
                value["full_name"].as_str()?,
                value["clone_url"].as_str()?,
                value["private"].as_bool().unwrap_or(false),
            ),
            Site::GitLab => (
                value["path_with_namespace"].as_str()?,
                value["http_url_to_repo"].as_str()?,
                value["visibility"].as_str() != Some("public"),
            ),
        };
        Some(Repo {
            name: name.to_owned(),
            clone_url: url.to_owned(),
            private,
            description: value["description"].as_str().unwrap_or_default().to_owned(),
            forge: self.site.name(),
        })
    }

    /// Your repositories, the most lately worked on first (at most 500).
    pub fn repos(&self, token: &str) -> Result<Vec<Repo>, String> {
        let mut found = Vec::new();
        for page in 1..=5 {
            let url = match self.site {
                Site::GitHub => format!(
                    "{}/user/repos?per_page=100&sort=pushed&page={page}",
                    self.api()
                ),
                Site::GitLab => format!(
                    "{}/projects?membership=true&order_by=last_activity_at&per_page=100&page={page}",
                    self.api()
                ),
            };
            let value = self.get(token, &url)?;
            let Some(list) = value.as_array() else { break };
            found.extend(list.iter().filter_map(|value| self.repo(value)));
            if list.len() < 100 {
                break;
            }
        }
        Ok(found)
    }

    /// Make the repository `name` on your account: private, unless not.
    pub fn create(&self, token: &str, name: &str, private: bool) -> Result<Repo, String> {
        let (url, body) = match self.site {
            Site::GitHub => (
                format!("{}/user/repos", self.api()),
                serde_json::json!({ "name": name, "private": private }),
            ),
            Site::GitLab => (
                format!("{}/projects", self.api()),
                serde_json::json!({
                    "name": name,
                    "visibility": if private { "private" } else { "public" },
                }),
            ),
        };
        let value = self.json(
            agent()
                .post(&url)
                .header("Authorization", bearer(token).as_str())
                .header("Accept", "application/json")
                .send_json(&body),
        )?;
        self.repo(&value)
            .ok_or_else(|| format!("{} didn't say what it made", self.name()))
    }

    fn attributes<'a>(&'a self, login: &'a str) -> HashMap<&'a str, &'a str> {
        [
            ("xdg:schema", "org.enso.Account"),
            ("service", self.host.as_str()),
            ("account", login),
        ]
        .into_iter()
        .collect()
    }

    /// Keep an account's tokens in the keyring.
    pub fn keep(&self, login: &str, tokens: &Tokens) -> Result<(), String> {
        enso_sys::secrets::store(
            &format!("{} ({login})", self.name()),
            &self.attributes(login),
            tokens.kept().as_bytes(),
        )
        .map_err(|err| format!("The keyring wouldn't keep it: {err}"))
    }

    /// The token to use for `login`, unlocking the keyring if need be —
    /// and, if it's running out, a fresh one from the app `client_id`,
    /// kept in its place.
    pub fn token(&self, login: &str, client_id: &str) -> Option<Zeroizing<String>> {
        let secret = enso_sys::secrets::lookup(&self.attributes(login)).ok()??;
        let tokens = Tokens::from_kept(std::str::from_utf8(&secret).ok()?)?;
        let now = jiff::Timestamp::now().as_second();
        match (&tokens.refresh, tokens.expires) {
            (Some(refresh), Some(expires)) if expires - EARLY <= now => {
                let fresh = self.refresh(client_id, refresh).ok()?;
                let _ = self.keep(login, &fresh);
                Some(fresh.access)
            }
            _ => Some(tokens.access),
        }
    }

    pub fn forget(&self, login: &str) -> Result<(), String> {
        enso_sys::secrets::clear(&self.attributes(login)).map_err(|err| err.to_string())
    }

    /// The git setting that asks enso-files for this forge's way in.
    fn helper(&self) -> String {
        format!("credential.https://{}.helper", self.host)
    }

    /// Have git ask this program for its way in to this forge (push, pull,
    /// clone) — or stop. On, it replaces whatever helper git had for it
    /// (gh's, say); off, it takes its own away.
    pub fn use_for_git(&self, on: bool) -> Result<(), String> {
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .output()
                .map_err(|err| format!("git can't be run: {err}"))
        };
        let helper = self.helper();
        // The installed name, so a rebuild needn't change git's setting.
        let program = enso_core::exe::installed();
        let ours = format!("!{} git-credential", program.display());
        let _ = git(&["config", "--global", "--unset-all", &helper]);
        if on {
            // An empty helper first: git forgets the ones set before.
            git(&["config", "--global", "--add", &helper, ""])?;
            git(&["config", "--global", "--add", &helper, &ours])?;
        }
        Ok(())
    }

    /// Whether git asks this program for this forge's way in.
    pub fn used_for_git(&self) -> bool {
        std::process::Command::new("git")
            .args(["config", "--global", "--get-all", &self.helper()])
            .output()
            .is_ok_and(|output| {
                let said = String::from_utf8_lossy(&output.stdout);
                said.contains("git-credential") && said.contains("enso-files")
            })
    }

    /// The user name git gives the forge with a token for `login`.
    fn credential_user<'a>(&self, login: &'a str) -> &'a str {
        match self.site {
            Site::GitHub => login,
            // GitLab takes OAuth tokens as the password of `oauth2`.
            Site::GitLab => "oauth2",
        }
    }
}

fn bearer(token: &str) -> Zeroizing<String> {
    Zeroizing::new(format!("Bearer {token}"))
}

/// An account signed in to: where, as whom, and with which app.
#[derive(Clone, Debug)]
pub struct Account {
    pub forge: Forge,
    pub login: String,
    pub client_id: String,
}

/// `enso-files git-credential get`: git asks for a way in to a host; for
/// one of `accounts`' forges, the account's user and token. Anything else,
/// and `store` and `erase` (the token is kept here, not by git), say
/// nothing — git asks the next helper.
pub fn git_credential(operation: &str, accounts: &[Account]) {
    if operation != "get" {
        return;
    }
    let mut asked = HashMap::new();
    for line in std::io::stdin().lines().map_while(Result::ok) {
        if line.is_empty() {
            break;
        }
        if let Some((key, value)) = line.split_once('=') {
            asked.insert(key.to_owned(), value.to_owned());
        }
    }
    let Some(host) = asked.get("host") else {
        return;
    };
    let Some(account) = accounts.iter().find(|account| &account.forge.host == host) else {
        return;
    };
    if let Some(token) = account.forge.token(&account.login, &account.client_id) {
        println!(
            "username={}\npassword={}",
            account.forge.credential_user(&account.login),
            token.as_str()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repositories_read_from_each_forge() {
        let value: Value = serde_json::from_str(
            r#"{"full_name":"me/enso","clone_url":"https://github.com/me/enso.git","private":true,"description":null}"#,
        )
        .unwrap();
        assert_eq!(
            Forge::github().repo(&value),
            Some(Repo {
                name: "me/enso".into(),
                clone_url: "https://github.com/me/enso.git".into(),
                private: true,
                description: String::new(),
                forge: "GitHub",
            })
        );
        assert!(
            Forge::github()
                .repo(&serde_json::json!({"name": "no full name"}))
                .is_none()
        );
        let project = serde_json::json!({
            "path_with_namespace": "me/sumi",
            "http_url_to_repo": "https://gitlab.com/me/sumi.git",
            "visibility": "internal",
            "description": "dotfiles",
        });
        let repo = Forge::gitlab("").repo(&project).unwrap();
        assert_eq!((repo.name.as_str(), repo.private), ("me/sumi", true));
        assert_eq!(repo.forge, "GitLab");
    }

    #[test]
    fn gitlab_is_where_its_host_says() {
        assert_eq!(Forge::gitlab("").host, "gitlab.com");
        assert_eq!(
            Forge::gitlab("https://git.example.org/").host,
            "git.example.org"
        );
        assert_eq!(Forge::gitlab("gitlab.com").name(), "GitLab");
        assert_eq!(
            Forge::gitlab("git.example.org").name(),
            "GitLab (git.example.org)"
        );
        assert_eq!(
            Forge::gitlab("git.example.org").api(),
            "https://git.example.org/api/v4"
        );
    }

    #[test]
    fn tokens_keep_and_come_back() {
        let bare = Tokens::from_kept("gho_abc").unwrap();
        assert_eq!(bare.access.as_str(), "gho_abc");
        assert!(bare.refresh.is_none() && bare.expires.is_none());
        let answer = serde_json::json!({
            "access_token": "glo_a",
            "refresh_token": "glr_b",
            "expires_in": 7200,
            "created_at": 1_000,
        });
        let tokens = Tokens::from_answer(&answer).unwrap();
        assert_eq!(tokens.expires, Some(8_200));
        let back = Tokens::from_kept(&tokens.kept()).unwrap();
        assert_eq!(back, tokens);
    }

    /// The forges, for real: a made-up app is refused, in their words.
    #[test]
    #[ignore = "reaches github.com and gitlab.com"]
    fn forges_answer() {
        for forge in [Forge::github(), Forge::gitlab("")] {
            let refused = forge.begin("enso-files-made-up-client").unwrap_err();
            assert!(refused.starts_with(forge.site.name()), "{refused}");
            eprintln!("{refused}");
        }
    }
}
