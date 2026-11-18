//! The Secret Service (`org.freedesktop.secrets`), the D-Bus API programs
//! keep their passwords and tokens through: gh, git (with its libsecret
//! helper), VS Code, Chromium, and anything built on libsecret.
//!
//! One collection, "login" (also the "default" alias), holds everything.
//! Every collection and item is an object on the bus. While locked, items
//! can be found — a program learns its secret is there and asks to unlock
//! — but not read. Unlocking goes through a prompt object, as the spec has
//! it: the program asks, the card comes up, the program hears when it's
//! done. Secrets travel in the clear between programs on the session bus
//! (the "plain" session), as they do with gnome-keyring's default.

use std::{
    collections::{BTreeMap, HashMap},
    sync::{Arc, Mutex, MutexGuard, PoisonError},
    thread,
};

use zbus::{
    Connection, OwnedObjectPath, OwnedVariant, Variant, interface, message::Header,
    object_server::SignalEmitter,
};
use zeroize::Zeroizing;

use crate::{
    asking::{self, Asked},
    store::{self, Item, Keyring},
};

pub const NAME: &str = "org.freedesktop.secrets";
pub const ROOT: &str = "/org/freedesktop/secrets";
const LOGIN: &str = "login";

/// Everything the objects share.
#[derive(Debug)]
pub struct Shared {
    /// `None` until the first unlock makes it.
    pub keyring: Option<Keyring>,
    next: u64,
    /// A prompt is up: no second one at once.
    prompting: bool,
    /// The new login password, while the old one is asked for.
    pending_new: Option<Zeroizing<Vec<u8>>>,
}

pub type State = Arc<Mutex<Shared>>;

fn lock(state: &State) -> MutexGuard<'_, Shared> {
    state.lock().unwrap_or_else(PoisonError::into_inner)
}

pub fn collection_path(name: &str) -> String {
    format!("{ROOT}/collection/{name}")
}

pub fn item_path(collection: &str, id: u64) -> String {
    format!("{ROOT}/collection/{collection}/{id}")
}

fn path(text: &str) -> OwnedObjectPath {
    OwnedObjectPath::try_from(text.to_owned())
        .unwrap_or_else(|_| OwnedObjectPath::try_from("/").unwrap_or_default())
}

fn root_path() -> OwnedObjectPath {
    path("/")
}

/// `(collection, id)` from an item's path.
fn parse_item(text: &str) -> Option<(String, u64)> {
    let rest = text.strip_prefix(&format!("{ROOT}/collection/"))?;
    let (collection, id) = rest.split_once('/')?;
    Some((collection.to_owned(), id.parse().ok()?))
}

#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.freedesktop.Secret.Error")]
pub enum Error {
    #[zbus(error)]
    ZBus(zbus::Error),
    IsLocked(String),
    NoSession(String),
    NoSuchObject(String),
}

/// `(session, parameters, value, content type)`, as the spec has a secret.
type Secret = (OwnedObjectPath, Vec<u8>, Vec<u8>, String);

/// Put every collection and item in the index on the bus.
pub async fn publish(connection: &Connection, state: &State) -> zbus::Result<()> {
    let server = connection.object_server();
    server
        .at(
            ROOT,
            Service {
                state: state.clone(),
            },
        )
        .await?;
    let names: Vec<String>;
    let items: Vec<(String, u64)>;
    {
        let shared = lock(state);
        let mut found: Vec<String> = shared
            .keyring
            .as_ref()
            .map(|keyring| keyring.index.keys().cloned().collect())
            .unwrap_or_default();
        if !found.iter().any(|name| name == LOGIN) {
            found.push(LOGIN.to_owned());
        }
        names = found;
        items = shared
            .keyring
            .as_ref()
            .map(|keyring| {
                keyring
                    .index
                    .iter()
                    .flat_map(|(name, items)| items.keys().map(move |id| (name.clone(), *id)))
                    .collect()
            })
            .unwrap_or_default();
    }
    for name in names {
        let collection = CollectionObject {
            state: state.clone(),
            name: name.clone(),
        };
        if name == LOGIN {
            server
                .at(
                    format!("{ROOT}/aliases/default"),
                    CollectionObject {
                        state: state.clone(),
                        name: name.clone(),
                    },
                )
                .await?;
        }
        server.at(collection_path(&name), collection).await?;
    }
    for (collection, id) in items {
        server
            .at(
                item_path(&collection, id),
                ItemObject {
                    state: state.clone(),
                    collection,
                    id,
                },
            )
            .await?;
    }
    Ok(())
}

pub struct Service {
    state: State,
}

#[interface(name = "org.freedesktop.Secret.Service")]
impl Service {
    async fn open_session(
        &self,
        algorithm: String,
        _input: OwnedVariant,
        #[zbus(connection)] connection: &Connection,
    ) -> zbus::fdo::Result<(OwnedVariant, OwnedObjectPath)> {
        if algorithm != "plain" {
            // libsecret asks for an encrypted session first, then plain.
            return Err(zbus::fdo::Error::NotSupported(
                "only plain sessions".to_owned(),
            ));
        }
        let number = {
            let mut shared = lock(&self.state);
            shared.next += 1;
            shared.next
        };
        let session = format!("{ROOT}/session/{number}");
        connection
            .object_server()
            .at(session.as_str(), Session)
            .await?;
        Ok((OwnedVariant::try_from(Variant::from(""))?, path(&session)))
    }

    async fn create_collection(
        &self,
        _properties: HashMap<String, OwnedVariant>,
        _alias: String,
    ) -> (OwnedObjectPath, OwnedObjectPath) {
        // One keyring, one collection: whatever's asked for is the login one.
        (path(&collection_path(LOGIN)), root_path())
    }

    async fn search_items(
        &self,
        attributes: HashMap<String, String>,
    ) -> (Vec<OwnedObjectPath>, Vec<OwnedObjectPath>) {
        let shared = lock(&self.state);
        let Some(keyring) = &shared.keyring else {
            return (Vec::new(), Vec::new());
        };
        let wanted: BTreeMap<String, String> = attributes.into_iter().collect();
        let found: Vec<OwnedObjectPath> = keyring
            .search(None, &wanted)
            .into_iter()
            .map(|(collection, id)| path(&item_path(&collection, id)))
            .collect();
        if keyring.is_unlocked() {
            (found, Vec::new())
        } else {
            (Vec::new(), found)
        }
    }

    async fn unlock(
        &self,
        objects: Vec<OwnedObjectPath>,
        #[zbus(connection)] connection: &Connection,
        #[zbus(header)] header: Header<'_>,
    ) -> zbus::fdo::Result<(Vec<OwnedObjectPath>, OwnedObjectPath)> {
        let unlocked = lock(&self.state)
            .keyring
            .as_ref()
            .is_some_and(Keyring::is_unlocked);
        if unlocked {
            return Ok((objects, root_path()));
        }
        let asker = asker(connection, &header).await;
        let number = {
            let mut shared = lock(&self.state);
            shared.next += 1;
            shared.next
        };
        let prompt = format!("{ROOT}/prompt/{number}");
        connection
            .object_server()
            .at(
                prompt.as_str(),
                Prompt {
                    state: self.state.clone(),
                    objects,
                    asker,
                    path: prompt.clone(),
                },
            )
            .await?;
        Ok((Vec::new(), path(&prompt)))
    }

    async fn lock(&self, objects: Vec<OwnedObjectPath>) -> (Vec<OwnedObjectPath>, OwnedObjectPath) {
        if let Some(keyring) = &mut lock(&self.state).keyring {
            keyring.lock();
        }
        (objects, root_path())
    }

    async fn get_secrets(
        &self,
        items: Vec<OwnedObjectPath>,
        session: OwnedObjectPath,
    ) -> Result<HashMap<OwnedObjectPath, Secret>, Error> {
        let shared = lock(&self.state);
        let keyring = shared
            .keyring
            .as_ref()
            .filter(|keyring| keyring.is_unlocked())
            .ok_or_else(|| Error::IsLocked("the keyring is locked".to_owned()))?;
        let mut secrets = HashMap::new();
        for item in items {
            let Some((collection, id)) = parse_item(item.as_str()) else {
                continue;
            };
            if let Some(found) = keyring
                .contents
                .collections
                .get(&collection)
                .and_then(|c| c.items.get(&id))
            {
                secrets.insert(
                    item,
                    (
                        session.clone(),
                        Vec::new(),
                        found.secret.to_vec(),
                        found.content_type.clone(),
                    ),
                );
            }
        }
        Ok(secrets)
    }

    async fn read_alias(&self, name: String) -> OwnedObjectPath {
        if name == "default" || name == LOGIN {
            path(&collection_path(LOGIN))
        } else {
            root_path()
        }
    }

    async fn set_alias(&self, _name: String, _collection: OwnedObjectPath) {}

    #[zbus(property)]
    async fn collections(&self) -> Vec<OwnedObjectPath> {
        vec![path(&collection_path(LOGIN))]
    }

    #[zbus(signal)]
    async fn collection_changed(
        emitter: &SignalEmitter<'_>,
        collection: OwnedObjectPath,
    ) -> zbus::Result<()>;
}

/// Which program is asking, for the card: its command's name.
async fn asker(connection: &Connection, header: &Header<'_>) -> String {
    let Some(sender) = header.sender() else {
        return String::new();
    };
    let pid: Option<u32> = connection
        .call_method(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            Some("org.freedesktop.DBus"),
            "GetConnectionUnixProcessID",
            &(sender.as_str(),),
        )
        .await
        .ok()
        .and_then(|reply| reply.body().deserialize().ok());
    pid.and_then(|pid| std::fs::read_to_string(format!("/proc/{pid}/comm")).ok())
        .map(|name| name.trim().to_owned())
        .unwrap_or_default()
}

pub struct Session;

#[interface(name = "org.freedesktop.Secret.Session")]
impl Session {
    async fn close(
        &self,
        #[zbus(connection)] connection: &Connection,
        #[zbus(header)] header: Header<'_>,
    ) {
        if let Some(here) = header.path() {
            let _ = connection
                .object_server()
                .remove::<Self, _>(here.to_owned())
                .await;
        }
    }
}

pub struct CollectionObject {
    state: State,
    name: String,
}

#[interface(name = "org.freedesktop.Secret.Collection")]
impl CollectionObject {
    async fn delete(&self) -> Result<OwnedObjectPath, Error> {
        Err(Error::NoSuchObject("the login collection stays".to_owned()))
    }

    async fn search_items(&self, attributes: HashMap<String, String>) -> Vec<OwnedObjectPath> {
        let shared = lock(&self.state);
        let wanted: BTreeMap<String, String> = attributes.into_iter().collect();
        shared
            .keyring
            .as_ref()
            .map(|keyring| {
                keyring
                    .search(Some(&self.name), &wanted)
                    .into_iter()
                    .map(|(collection, id)| path(&item_path(&collection, id)))
                    .collect()
            })
            .unwrap_or_default()
    }

    async fn create_item(
        &self,
        properties: HashMap<String, OwnedVariant>,
        secret: Secret,
        replace: bool,
        #[zbus(connection)] connection: &Connection,
    ) -> Result<(OwnedObjectPath, OwnedObjectPath), Error> {
        let label = properties
            .get("org.freedesktop.Secret.Item.Label")
            .and_then(|value| String::try_from(value.try_clone().ok()?).ok())
            .unwrap_or_default();
        let attributes: BTreeMap<String, String> = properties
            .get("org.freedesktop.Secret.Item.Attributes")
            .and_then(|value| HashMap::<String, String>::try_from(value.try_clone().ok()?).ok())
            .unwrap_or_default()
            .into_iter()
            .collect();
        let (_, _, value, content_type) = secret;
        let now = store::now();
        let item = Item {
            label,
            attributes,
            secret: Zeroizing::new(value),
            content_type,
            created: now,
            modified: now,
        };
        let id = {
            let mut shared = lock(&self.state);
            let keyring = shared
                .keyring
                .as_mut()
                .filter(|keyring| keyring.is_unlocked())
                .ok_or_else(|| Error::IsLocked("unlock the keyring first".to_owned()))?;
            keyring
                .store(&self.name, item, replace)
                .map_err(Error::NoSuchObject)?
        };
        let item_path = item_path(&self.name, id);
        // Replacing keeps the object that's already there.
        let _ = connection
            .object_server()
            .at(
                item_path.as_str(),
                ItemObject {
                    state: self.state.clone(),
                    collection: self.name.clone(),
                    id,
                },
            )
            .await;
        if let Ok(emitter) = SignalEmitter::new(connection, collection_path(&self.name)) {
            let _ = Self::item_created(&emitter, path(&item_path)).await;
        }
        Ok((path(&item_path), root_path()))
    }

    #[zbus(property)]
    async fn items(&self) -> Vec<OwnedObjectPath> {
        let shared = lock(&self.state);
        shared
            .keyring
            .as_ref()
            .and_then(|keyring| keyring.index.get(&self.name))
            .map(|items| {
                items
                    .keys()
                    .map(|id| path(&item_path(&self.name, *id)))
                    .collect()
            })
            .unwrap_or_default()
    }

    #[zbus(property)]
    async fn label(&self) -> String {
        let shared = lock(&self.state);
        shared
            .keyring
            .as_ref()
            .and_then(|keyring| keyring.contents.collections.get(&self.name))
            .map_or_else(|| "Login".to_owned(), |collection| collection.label.clone())
    }

    #[zbus(property)]
    async fn locked(&self) -> bool {
        !lock(&self.state)
            .keyring
            .as_ref()
            .is_some_and(Keyring::is_unlocked)
    }

    #[zbus(property)]
    async fn created(&self) -> u64 {
        lock(&self.state)
            .keyring
            .as_ref()
            .and_then(|keyring| keyring.contents.collections.get(&self.name))
            .map_or(0, |collection| collection.created)
    }

    #[zbus(property)]
    async fn modified(&self) -> u64 {
        lock(&self.state)
            .keyring
            .as_ref()
            .and_then(|keyring| keyring.contents.collections.get(&self.name))
            .map_or(0, |collection| collection.modified)
    }

    #[zbus(signal)]
    async fn item_created(emitter: &SignalEmitter<'_>, item: OwnedObjectPath) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn item_deleted(emitter: &SignalEmitter<'_>, item: OwnedObjectPath) -> zbus::Result<()>;
}

pub struct ItemObject {
    state: State,
    collection: String,
    id: u64,
}

impl ItemObject {
    /// This item, if the keyring is open.
    fn with<R>(&self, read: impl FnOnce(&Item) -> R) -> Option<R> {
        let shared = lock(&self.state);
        shared
            .keyring
            .as_ref()
            .filter(|keyring| keyring.is_unlocked())
            .and_then(|keyring| keyring.contents.collections.get(&self.collection))
            .and_then(|collection| collection.items.get(&self.id))
            .map(read)
    }

    fn change(&self, change: impl FnOnce(&mut Item)) -> Result<(), Error> {
        let mut shared = lock(&self.state);
        let keyring = shared
            .keyring
            .as_mut()
            .filter(|keyring| keyring.is_unlocked())
            .ok_or_else(|| Error::IsLocked("the keyring is locked".to_owned()))?;
        let item = keyring
            .contents
            .collections
            .get_mut(&self.collection)
            .and_then(|collection| collection.items.get_mut(&self.id))
            .ok_or_else(|| Error::NoSuchObject("gone".to_owned()))?;
        change(item);
        item.modified = store::now();
        keyring
            .save()
            .map_err(|err| Error::NoSuchObject(err.to_string()))
    }
}

#[interface(name = "org.freedesktop.Secret.Item")]
impl ItemObject {
    async fn delete(
        &self,
        #[zbus(connection)] connection: &Connection,
    ) -> Result<OwnedObjectPath, Error> {
        {
            let mut shared = lock(&self.state);
            let keyring = shared
                .keyring
                .as_mut()
                .filter(|keyring| keyring.is_unlocked())
                .ok_or_else(|| Error::IsLocked("the keyring is locked".to_owned()))?;
            if let Some(collection) = keyring.contents.collections.get_mut(&self.collection) {
                collection.items.remove(&self.id);
            }
            keyring
                .save()
                .map_err(|err| Error::NoSuchObject(err.to_string()))?;
        }
        let here = item_path(&self.collection, self.id);
        if let Ok(emitter) = SignalEmitter::new(connection, collection_path(&self.collection)) {
            let _ = CollectionObject::item_deleted(&emitter, path(&here)).await;
        }
        let _ = connection.object_server().remove::<Self, _>(here).await;
        Ok(root_path())
    }

    async fn get_secret(&self, session: OwnedObjectPath) -> Result<Secret, Error> {
        self.with(|item| {
            (
                session.clone(),
                Vec::new(),
                item.secret.to_vec(),
                item.content_type.clone(),
            )
        })
        .ok_or_else(|| Error::IsLocked("the keyring is locked".to_owned()))
    }

    async fn set_secret(&self, secret: Secret) -> Result<(), Error> {
        let (_, _, value, content_type) = secret;
        self.change(|item| {
            item.secret = Zeroizing::new(value);
            item.content_type = content_type;
        })
    }

    #[zbus(property)]
    async fn locked(&self) -> bool {
        self.with(|_| ()).is_none()
    }

    #[zbus(property)]
    async fn attributes(&self) -> HashMap<String, String> {
        self.with(|item| item.attributes.clone().into_iter().collect())
            .unwrap_or_default()
    }

    #[zbus(property)]
    async fn set_attributes(
        &mut self,
        attributes: HashMap<String, String>,
    ) -> zbus::fdo::Result<()> {
        self.change(|item| item.attributes = attributes.into_iter().collect())
            .map_err(|err| zbus::fdo::Error::Failed(format!("{err:?}")))
    }

    #[zbus(property)]
    async fn label(&self) -> String {
        self.with(|item| item.label.clone()).unwrap_or_default()
    }

    #[zbus(property)]
    async fn set_label(&mut self, label: String) -> zbus::fdo::Result<()> {
        self.change(|item| item.label = label)
            .map_err(|err| zbus::fdo::Error::Failed(format!("{err:?}")))
    }

    #[zbus(property)]
    async fn created(&self) -> u64 {
        self.with(|item| item.created).unwrap_or(0)
    }

    #[zbus(property)]
    async fn modified(&self) -> u64 {
        self.with(|item| item.modified).unwrap_or(0)
    }
}

/// Unlocking, the spec's way: the program calls Prompt, the card comes
/// up, and Completed says how it went.
pub struct Prompt {
    state: State,
    objects: Vec<OwnedObjectPath>,
    asker: String,
    path: String,
}

#[interface(name = "org.freedesktop.Secret.Prompt")]
impl Prompt {
    async fn prompt(&self, _window_id: String, #[zbus(connection)] connection: &Connection) {
        let (state, objects, asker, here) = (
            self.state.clone(),
            self.objects.clone(),
            self.asker.clone(),
            self.path.clone(),
        );
        let connection = connection.clone();
        let _ = thread::Builder::new()
            .name("enso-keyring-prompt".to_owned())
            .spawn(move || {
                let unlocked = unlock_with_card(&state, &asker);
                zbus::block_on(async {
                    if let Ok(emitter) = SignalEmitter::new(&connection, here.as_str()) {
                        let objects = if unlocked { objects } else { Vec::new() };
                        let result = OwnedVariant::try_from(Variant::from(objects)).ok();
                        if let Some(result) = result {
                            let _ = Self::completed(&emitter, !unlocked, result).await;
                        }
                    }
                    let _ = connection
                        .object_server()
                        .remove::<Self, _>(here.as_str())
                        .await;
                });
            });
    }

    async fn dismiss(&self, #[zbus(connection)] connection: &Connection) {
        if let Ok(emitter) = SignalEmitter::new(connection, self.path.as_str())
            && let Ok(result) = OwnedVariant::try_from(Variant::from(Vec::<OwnedObjectPath>::new()))
        {
            let _ = Self::completed(&emitter, true, result).await;
        }
    }

    #[zbus(signal)]
    async fn completed(
        emitter: &SignalEmitter<'_>,
        dismissed: bool,
        result: OwnedVariant,
    ) -> zbus::Result<()>;
}

/// Bring up the card and unlock — or make the keyring, the first time.
/// Returns whether it's unlocked now.
pub fn unlock_with_card(state: &State, asker: &str) -> bool {
    {
        let mut shared = lock(state);
        if shared.keyring.as_ref().is_some_and(Keyring::is_unlocked) {
            return true;
        }
        if shared.prompting {
            return false;
        }
        shared.prompting = true;
    }
    let first = lock(state).keyring.is_none();
    let mode = if first {
        asking::Mode::Create
    } else {
        asking::Mode::Unlock
    };
    let state_for = state.clone();
    let result = asking::ask(mode, asker, move |asked| answer(&state_for, asked));
    lock(state).prompting = false;
    result
}

/// What the card's answer comes to: `Ok` closes it, `Err` asks again
/// (with what to say, and in which mode).
fn answer(state: &State, asked: Asked) -> Result<(), (String, asking::Mode)> {
    let not_login = || {
        (
            "That's not your login password".to_owned(),
            asking::Mode::Unlock,
        )
    };
    match asked {
        Asked::Password(password) => {
            let mut shared = lock(state);
            match &mut shared.keyring {
                None => {
                    // The first time: the login password, checked with the system.
                    if !enso_sys::auth::verify(&password).unwrap_or(false) {
                        return Err((
                            "That's not your login password".to_owned(),
                            asking::Mode::Create,
                        ));
                    }
                    let keyring =
                        Keyring::create(&store::path(), &password, crate::crypto::Cost::default())
                            .map_err(|why| {
                                (
                                    format!("Couldn't make the keyring: {why}"),
                                    asking::Mode::Create,
                                )
                            })?;
                    shared.keyring = Some(keyring);
                    Ok(())
                }
                Some(keyring) => match keyring.unlock(&password) {
                    Ok(()) => Ok(()),
                    Err(store::Refused::Damaged(why)) => Err((
                        format!("The keyring doesn't read: {why}"),
                        asking::Mode::Unlock,
                    )),
                    Err(store::Refused::Password) => {
                        // Not what locked it — but is it the login password
                        // now? Then the password changed since: ask for the
                        // one before.
                        if enso_sys::auth::verify(&password).unwrap_or(false) {
                            shared.pending_new = Some(password);
                            Err((String::new(), asking::Mode::OldPassword))
                        } else {
                            Err(not_login())
                        }
                    }
                },
            }
        }
        Asked::Old(old) => {
            let mut shared = lock(state);
            let new = shared.pending_new.take();
            let Some(keyring) = &mut shared.keyring else {
                return Err(not_login());
            };
            match keyring.unlock(&old) {
                Ok(()) => {
                    if let Some(new) = new {
                        let _ = keyring.rewrap(&new);
                    }
                    Ok(())
                }
                Err(_) => {
                    shared.pending_new = new;
                    Err((
                        "That's not it either — the password before the last change".to_owned(),
                        asking::Mode::OldPassword,
                    ))
                }
            }
        }
        Asked::New(new) => {
            // The login password changed while unlocked: re-lock with it.
            if !enso_sys::auth::verify(&new).unwrap_or(false) {
                return Err((
                    "That's not your login password now".to_owned(),
                    asking::Mode::Changed,
                ));
            }
            let mut shared = lock(state);
            match &mut shared.keyring {
                Some(keyring) if keyring.is_unlocked() => keyring
                    .rewrap(&new)
                    .map_err(|why| (format!("Couldn't keep up: {why}"), asking::Mode::Changed)),
                _ => Ok(()),
            }
        }
    }
}

/// The login password changed while the keyring was open: ask for the
/// new one once, and lock the keyring with it.
pub fn follow_change(state: &State) {
    {
        let mut shared = lock(state);
        if shared.prompting {
            return;
        }
        shared.prompting = true;
    }
    let state_for = state.clone();
    asking::ask(asking::Mode::Changed, "", move |asked| {
        answer(&state_for, asked)
    });
    lock(state).prompting = false;
}

impl Shared {
    pub fn new(keyring: Option<Keyring>) -> Self {
        Self {
            keyring,
            next: 0,
            prompting: false,
            pending_new: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        io::{BufRead, BufReader, Write},
        process::{Command, Stdio},
    };

    use super::*;
    use crate::crypto::Cost;

    /// A bus of its own, so the test touches nothing of the session's.
    struct Bus {
        daemon: std::process::Child,
        address: String,
    }

    impl Drop for Bus {
        fn drop(&mut self) {
            let _ = self.daemon.kill();
            let _ = self.daemon.wait();
        }
    }

    fn private_bus() -> Option<Bus> {
        let mut daemon = Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--print-address=1"])
            .stdout(Stdio::piped())
            .spawn()
            .ok()?;
        let mut address = String::new();
        BufReader::new(daemon.stdout.take()?)
            .read_line(&mut address)
            .ok()?;
        Some(Bus {
            daemon,
            address: address.trim().to_owned(),
        })
    }

    fn secret_tool(bus: &Bus, args: &[&str], input: Option<&str>) -> (bool, String) {
        let mut child = Command::new("secret-tool")
            .args(args)
            .env("DBUS_SESSION_BUS_ADDRESS", &bus.address)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        if let (Some(input), Some(mut stdin)) = (input, child.stdin.take()) {
            stdin.write_all(input.as_bytes()).unwrap();
        }
        let output = child.wait_with_output().unwrap();
        (
            output.status.success(),
            format!(
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            ),
        )
    }

    #[test]
    fn libsecret_keeps_finds_and_forgets_through_it() {
        let (Some(bus), true) = (
            private_bus(),
            Command::new("secret-tool")
                .arg("--version")
                .output()
                .is_ok(),
        ) else {
            eprintln!("no dbus-daemon or secret-tool here: skipped");
            return;
        };
        let dir = std::env::temp_dir().join(format!("enso-keyring-bus-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let light = Cost {
            memory: 1024,
            passes: 1,
        };
        let keyring = Keyring::create(&dir.join("keyring.toml"), b"test password", light).unwrap();
        let state: State = Arc::new(Mutex::new(Shared::new(Some(keyring))));
        let connection = zbus::block_on(async {
            let connection = zbus::connection::Builder::address(bus.address.as_str())
                .build()
                .await?;
            connection.request_name(NAME).await?;
            publish(&connection, &state).await?;
            zbus::Result::Ok(connection)
        })
        .unwrap();

        let (stored, said) = secret_tool(
            &bus,
            &[
                "store",
                "--label=GitHub token",
                "service",
                "github.com",
                "user",
                "ren",
            ],
            Some("gho_example"),
        );
        assert!(stored, "store: {said}");
        let (found, secret) = secret_tool(&bus, &["lookup", "service", "github.com"], None);
        assert!(found, "lookup: {secret}");
        assert_eq!(secret.trim(), "gho_example");
        // Stored again: replaced, not doubled.
        secret_tool(
            &bus,
            &[
                "store",
                "--label=GitHub token",
                "service",
                "github.com",
                "user",
                "ren",
            ],
            Some("gho_newer"),
        );
        assert_eq!(
            secret_tool(&bus, &["lookup", "user", "ren"], None).1.trim(),
            "gho_newer"
        );
        assert_eq!(
            lock(&state).keyring.as_ref().unwrap().index["login"].len(),
            1
        );

        // Locked: found, but not given out (the card can't show in a test,
        // so the unlock is put away).
        lock(&state).keyring.as_mut().unwrap().lock();
        let (found, _) = secret_tool(&bus, &["lookup", "service", "github.com"], None);
        assert!(!found, "a locked keyring gave its secret away");
        lock(&state)
            .keyring
            .as_mut()
            .unwrap()
            .unlock(b"test password")
            .unwrap();

        let (cleared, said) = secret_tool(&bus, &["clear", "service", "github.com"], None);
        assert!(cleared, "clear: {said}");
        assert!(!secret_tool(&bus, &["lookup", "service", "github.com"], None).0);
        drop(connection);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
