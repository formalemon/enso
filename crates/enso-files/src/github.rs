//! GitHub: signing in with the device flow, and what an account allows —
//! your repositories, to clone; a new one, to publish a folder to; and
//! being git's way in, for push and pull.
//!
//! Signing in never passes a password through here: GitHub shows a page
//! (`github.com/login/device`), you type the short code shown here, and
//! approve; this asks GitHub, every few seconds, whether you have, and is
//! given a token. It needs an OAuth app of your own — its client ID, a
//! public identifier, is all this knows of it. The token is kept in the
//! session's keyring (enso-keyring), never in a file.

use std::{collections::HashMap, time::Duration};

use serde_json::Value;
use zeroize::Zeroizing;

const SCOPE: &str = "repo read:user";
const TIMEOUT: Duration = Duration::from_secs(20);

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(TIMEOUT))
        .user_agent("enso-files")
        // GitHub's errors come with words worth showing.
        .http_status_as_error(false)
        .build()
        .into()
}

fn json(response: Result<ureq::http::Response<ureq::Body>, ureq::Error>) -> Result<Value, String> {
    let mut response = response.map_err(|err| format!("GitHub can't be reached: {err}"))?;
    let status = response.status();
    let value: Value = response
        .body_mut()
        .read_json()
        .map_err(|err| format!("GitHub's answer doesn't read: {err}"))?;
    if status.is_client_error() || status.is_server_error() {
        let message = value["message"]
            .as_str()
            .or(value["error_description"].as_str())
            .map_or_else(|| format!("it said no ({status})"), str::to_owned);
        return Err(format!("GitHub: {message}"));
    }
    Ok(value)
}

/// What to show while signing in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceCode {
    pub device_code: String,
    /// What you type at GitHub: `ABCD-1234`.
    pub user_code: String,
    pub page: String,
    /// Seconds between asking whether it's done.
    pub interval: u64,
}

/// Begin signing in with the OAuth app `client_id`.
pub fn begin(client_id: &str) -> Result<DeviceCode, String> {
    let value = json(
        agent()
            .post("https://github.com/login/device/code")
            .header("Accept", "application/json")
            .send_form([("client_id", client_id), ("scope", SCOPE)]),
    )
    .map_err(|why| {
        if why.contains("404") {
            "GitHub doesn't know that client ID: check it on your OAuth app's page".to_owned()
        } else {
            why
        }
    })?;
    if let Some(error) = value["error_description"]
        .as_str()
        .or(value["error"].as_str())
    {
        return Err(format!("GitHub: {error}"));
    }
    Ok(DeviceCode {
        device_code: value["device_code"]
            .as_str()
            .ok_or("GitHub sent no code")?
            .to_owned(),
        user_code: value["user_code"]
            .as_str()
            .ok_or("GitHub sent no code")?
            .to_owned(),
        page: value["verification_uri"]
            .as_str()
            .unwrap_or("https://github.com/login/device")
            .to_owned(),
        interval: value["interval"].as_u64().unwrap_or(5),
    })
}

/// How signing in stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Waiting {
    /// Not approved yet.
    Pending,
    /// Asked too often: wait this many seconds more each time.
    SlowDown,
    Token(Zeroizing<String>),
    /// Declined, expired, or wrong: why.
    Failed(String),
}

/// Ask whether signing in has been approved.
pub fn poll(client_id: &str, device_code: &str) -> Waiting {
    let value = match json(
        agent()
            .post("https://github.com/login/oauth/access_token")
            .header("Accept", "application/json")
            .send_form([
                ("client_id", client_id),
                ("device_code", device_code),
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ]),
    ) {
        Ok(value) => value,
        // Offline for a moment: ask again.
        Err(_) => return Waiting::Pending,
    };
    if let Some(token) = value["access_token"].as_str() {
        return Waiting::Token(Zeroizing::new(token.to_owned()));
    }
    match value["error"].as_str() {
        Some("authorization_pending") => Waiting::Pending,
        Some("slow_down") => Waiting::SlowDown,
        Some("access_denied") => Waiting::Failed("It was declined at GitHub".to_owned()),
        Some("expired_token") => {
            Waiting::Failed("The code ran out of time: sign in again".to_owned())
        }
        Some("device_flow_disabled") => {
            Waiting::Failed("The OAuth app needs “Enable Device Flow” ticked, on GitHub".to_owned())
        }
        Some(other) => Waiting::Failed(format!(
            "GitHub: {}",
            value["error_description"].as_str().unwrap_or(other)
        )),
        None => Waiting::Pending,
    }
}

/// The `Authorization` header's value for `token`.
fn bearer(token: &str) -> Zeroizing<String> {
    Zeroizing::new(format!("Bearer {token}"))
}

fn get(token: &str, url: &str) -> Result<Value, String> {
    json(
        agent()
            .get(url)
            .header("Authorization", bearer(token).as_str())
            .header("Accept", "application/vnd.github+json")
            .call(),
    )
}

/// The account a token is for: its login.
pub fn login(token: &str) -> Result<String, String> {
    get(token, "https://api.github.com/user")?["login"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| "GitHub didn't say who you are".to_owned())
}

/// A repository of yours (or one you work on).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Repo {
    /// `owner/name`.
    pub name: String,
    pub clone_url: String,
    pub private: bool,
    pub description: String,
}

fn repo(value: &Value) -> Option<Repo> {
    Some(Repo {
        name: value["full_name"].as_str()?.to_owned(),
        clone_url: value["clone_url"].as_str()?.to_owned(),
        private: value["private"].as_bool().unwrap_or(false),
        description: value["description"].as_str().unwrap_or_default().to_owned(),
    })
}

/// Your repositories, the most lately pushed to first (at most 500).
pub fn repos(token: &str) -> Result<Vec<Repo>, String> {
    let mut found = Vec::new();
    for page in 1..=5 {
        let value = get(
            token,
            &format!("https://api.github.com/user/repos?per_page=100&sort=pushed&page={page}"),
        )?;
        let Some(list) = value.as_array() else { break };
        found.extend(list.iter().filter_map(repo));
        if list.len() < 100 {
            break;
        }
    }
    Ok(found)
}

/// Make the repository `name` on your account: private, unless not.
pub fn create(token: &str, name: &str, private: bool) -> Result<Repo, String> {
    let body = serde_json::json!({ "name": name, "private": private });
    let value = json(
        agent()
            .post("https://api.github.com/user/repos")
            .header("Authorization", bearer(token).as_str())
            .header("Accept", "application/vnd.github+json")
            .send_json(&body),
    )?;
    repo(&value).ok_or_else(|| "GitHub didn't say what it made".to_owned())
}

fn attributes(login: &str) -> HashMap<&str, &str> {
    [
        ("xdg:schema", "org.enso.Account"),
        ("service", "github.com"),
        ("account", login),
    ]
    .into_iter()
    .collect()
}

/// Keep the token in the keyring.
pub fn keep(login: &str, token: &str) -> Result<(), String> {
    enso_sys::secrets::store(
        &format!("GitHub ({login})"),
        &attributes(login),
        token.as_bytes(),
    )
    .map_err(|err| format!("The keyring wouldn't keep it: {err}"))
}

/// The token kept for `login`, unlocking the keyring if need be.
pub fn token(login: &str) -> Option<Zeroizing<String>> {
    let secret = enso_sys::secrets::lookup(&attributes(login)).ok()??;
    let text = std::str::from_utf8(&secret).ok()?;
    Some(Zeroizing::new(text.to_owned()))
}

pub fn forget(login: &str) -> Result<(), String> {
    enso_sys::secrets::clear(&attributes(login)).map_err(|err| err.to_string())
}

/// The git setting that asks enso-files for GitHub's way in.
const HELPER: &str = "credential.https://github.com.helper";

/// Have git ask this program for its way in to GitHub (push, pull,
/// clone) — or stop. On, it replaces whatever helper git had for GitHub
/// (gh's, say); off, it takes its own away.
pub fn use_for_git(on: bool) -> Result<(), String> {
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .output()
            .map_err(|err| format!("git can't be run: {err}"))
    };
    // The installed name, so a rebuild needn't change git's setting.
    let program = enso_core::exe::installed();
    let ours = format!("!{} git-credential", program.display());
    if on {
        // An empty helper first: git forgets the ones set before.
        let _ = git(&["config", "--global", "--unset-all", HELPER]);
        git(&["config", "--global", "--add", HELPER, ""])?;
        git(&["config", "--global", "--add", HELPER, &ours])?;
    } else {
        let _ = git(&["config", "--global", "--unset-all", HELPER]);
    }
    Ok(())
}

/// Whether git asks this program for GitHub's way in.
pub fn used_for_git() -> bool {
    std::process::Command::new("git")
        .args(["config", "--global", "--get-all", HELPER])
        .output()
        .is_ok_and(|output| {
            String::from_utf8_lossy(&output.stdout).contains("git-credential")
                && String::from_utf8_lossy(&output.stdout).contains("enso-files")
        })
}

/// `enso-files git-credential get`: git asks for a way in to a host; for
/// github.com, the account's login and token. Anything else, and `store`
/// and `erase` (the token is kept here, not by git), say nothing — git
/// asks the next helper.
pub fn git_credential(operation: &str, login: Option<&str>) {
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
    if asked.get("host").map(String::as_str) != Some("github.com") {
        return;
    }
    let Some(login) = login else { return };
    if let Some(token) = token(login) {
        println!("username={login}\npassword={}", token.as_str());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repositories_read_from_json() {
        let value: Value = serde_json::from_str(
            r#"{"full_name":"me/enso","clone_url":"https://github.com/me/enso.git","private":true,"description":null}"#,
        )
        .unwrap();
        assert_eq!(
            repo(&value),
            Some(Repo {
                name: "me/enso".into(),
                clone_url: "https://github.com/me/enso.git".into(),
                private: true,
                description: String::new(),
            })
        );
        assert!(repo(&serde_json::json!({"name": "no full name"})).is_none());
    }

    /// GitHub, for real: a made-up app is refused, in GitHub's words.
    #[test]
    #[ignore = "reaches github.com"]
    fn github_answers() {
        let refused = begin("enso-files-made-up-client").unwrap_err();
        assert!(refused.starts_with("GitHub"), "{refused}");
        eprintln!("{refused}");
    }
}
