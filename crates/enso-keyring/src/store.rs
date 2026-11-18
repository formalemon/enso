//! What the keyring keeps, and how it's kept on disk
//! (`~/.local/share/enso/keyring.toml`, readable by you alone).
//!
//! A random master key seals everything; the master key is kept sealed by
//! a key made from your login password. Changing the password re-seals the
//! master key and nothing else. Every collection, label, attribute, and
//! secret is in the sealed part. Beside it, each item's attributes are
//! kept hashed — as gnome-keyring keeps them — so a program can find its
//! secret while the keyring is locked, and so know to ask for it to be
//! unlocked; a hash says nothing of what's in it.

use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::{Zeroize, Zeroizing};

use crate::crypto::{self, Cost, Secret32};

/// One secret: a password, a token.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Item {
    pub label: String,
    pub attributes: BTreeMap<String, String>,
    /// The secret, hex in the sealed text.
    #[serde(with = "secret_hex")]
    pub secret: Zeroizing<Vec<u8>>,
    pub content_type: String,
    pub created: u64,
    pub modified: u64,
}

mod secret_hex {
    use serde::{Deserialize, Deserializer, Serializer};
    use zeroize::Zeroizing;

    pub fn serialize<S: Serializer>(secret: &Zeroizing<Vec<u8>>, to: S) -> Result<S::Ok, S::Error> {
        let text = Zeroizing::new(crate::crypto::hex(secret));
        to.serialize_str(&text)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(from: D) -> Result<Zeroizing<Vec<u8>>, D::Error> {
        let text = Zeroizing::new(String::deserialize(from)?);
        crate::crypto::unhex(&text)
            .map(Zeroizing::new)
            .ok_or_else(|| serde::de::Error::custom("a secret that isn't hex"))
    }
}

/// A named set of items: "login" is the one everything uses by default.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Collection {
    pub label: String,
    pub created: u64,
    pub modified: u64,
    /// By id, never reused.
    pub items: BTreeMap<u64, Item>,
}

/// Everything, as it is unlocked.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Contents {
    /// By name (the last part of its D-Bus path).
    pub collections: BTreeMap<String, Collection>,
    pub next_id: u64,
}

/// What's on disk.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct File {
    version: u32,
    cost: Cost,
    /// Hex: the password's salt, the sealed master key, the sealed contents.
    salt: String,
    master: String,
    contents: String,
    /// For each collection, each item's attribute hashes.
    #[serde(default)]
    index: BTreeMap<String, BTreeMap<String, Vec<String>>>,
}

/// What can be known while locked: which items there are, and the hashes
/// of their attributes.
pub type Index = BTreeMap<String, BTreeMap<u64, Vec<String>>>;

/// Why it wouldn't unlock.
#[derive(Debug, PartialEq, Eq)]
pub enum Refused {
    /// The password doesn't open it.
    Password,
    /// It doesn't read: damaged, or not a keyring.
    Damaged(String),
}

pub fn path() -> PathBuf {
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/share")
        });
    data.join("enso").join("keyring.toml")
}

/// `name=value`, hashed, for finding while locked.
pub fn hash(name: &str, value: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(name.as_bytes());
    digest.update([0]);
    digest.update(value.as_bytes());
    crypto::hex(&digest.finalize())
}

fn hashes(item: &Item) -> Vec<String> {
    item.attributes
        .iter()
        .map(|(name, value)| hash(name, value))
        .collect()
}

/// The keyring, locked or open.
#[derive(Debug)]
pub struct Keyring {
    path: PathBuf,
    cost: Cost,
    /// The master key, while unlocked.
    master: Option<Secret32>,
    pub contents: Contents,
    pub index: Index,
    /// The password's salt and the sealed master key, as on disk.
    salt: [u8; 16],
    sealed_master: Vec<u8>,
}

impl Keyring {
    /// The keyring at `path`, locked; `None` if there isn't one yet.
    pub fn load(path: &Path) -> io::Result<Option<Self>> {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(err),
        };
        let file: File = toml::from_str(&text).map_err(io::Error::other)?;
        let salt: [u8; 16] = crypto::unhex(&file.salt)
            .and_then(|salt| salt.try_into().ok())
            .ok_or_else(|| io::Error::other("the keyring's salt doesn't read"))?;
        let index = file
            .index
            .into_iter()
            .map(|(name, items)| {
                let items = items
                    .into_iter()
                    .filter_map(|(id, hashes)| Some((id.parse().ok()?, hashes)))
                    .collect();
                (name, items)
            })
            .collect();
        Ok(Some(Self {
            path: path.to_owned(),
            cost: file.cost,
            master: None,
            contents: Contents::default(),
            index,
            salt,
            sealed_master: crypto::unhex(&file.master).unwrap_or_default(),
        }))
    }

    /// A new, empty keyring locked by `password`, with a "login" collection;
    /// unlocked.
    pub fn create(path: &Path, password: &[u8], cost: Cost) -> Result<Self, String> {
        let master = Zeroizing::new(crypto::random::<32>());
        let salt = crypto::random::<16>();
        let wrapping = crypto::derive(password, &salt, cost)?;
        let mut keyring = Self {
            path: path.to_owned(),
            cost,
            sealed_master: crypto::seal(&wrapping, master.as_ref()),
            master: Some(master),
            contents: Contents::default(),
            index: Index::default(),
            salt,
        };
        let now = now();
        keyring.contents.collections.insert(
            "login".to_owned(),
            Collection {
                label: "Login".to_owned(),
                created: now,
                modified: now,
                items: BTreeMap::new(),
            },
        );
        keyring.save().map_err(|err| err.to_string())?;
        Ok(keyring)
    }

    pub fn is_unlocked(&self) -> bool {
        self.master.is_some()
    }

    /// Unlock with `password`.
    pub fn unlock(&mut self, password: &[u8]) -> Result<(), Refused> {
        let wrapping = crypto::derive(password, &self.salt, self.cost).map_err(Refused::Damaged)?;
        let master = crypto::open(&wrapping, &self.sealed_master).ok_or(Refused::Password)?;
        let master: [u8; 32] = master
            .as_slice()
            .try_into()
            .map_err(|_| Refused::Damaged("the master key is the wrong size".into()))?;
        let master = Zeroizing::new(master);
        let text =
            fs::read_to_string(&self.path).map_err(|err| Refused::Damaged(err.to_string()))?;
        let file: File = toml::from_str(&text).map_err(|err| Refused::Damaged(err.to_string()))?;
        let sealed = crypto::unhex(&file.contents)
            .ok_or_else(|| Refused::Damaged("contents don't read".into()))?;
        let plain = crypto::open(&master, &sealed)
            .ok_or_else(|| Refused::Damaged("the contents have been changed".into()))?;
        let text = std::str::from_utf8(&plain).map_err(|err| Refused::Damaged(err.to_string()))?;
        self.contents = toml::from_str(text).map_err(|err| Refused::Damaged(err.to_string()))?;
        self.master = Some(master);
        Ok(())
    }

    /// Forget the master key and the contents: locked again.
    pub fn lock(&mut self) {
        self.master = None;
        for collection in self.contents.collections.values_mut() {
            for item in collection.items.values_mut() {
                item.secret.zeroize();
            }
        }
        self.contents = Contents::default();
    }

    /// Lock the master key with a new password (the login password has
    /// changed). Unlocked only.
    pub fn rewrap(&mut self, password: &[u8]) -> Result<(), String> {
        let master = self.master.as_ref().ok_or("locked")?;
        let salt = crypto::random::<16>();
        let wrapping = crypto::derive(password, &salt, self.cost)?;
        self.sealed_master = crypto::seal(&wrapping, master.as_ref());
        self.salt = salt;
        self.save().map_err(|err| err.to_string())
    }

    /// Write it all, sealed. Unlocked only. Written beside it and put in
    /// place whole, readable by you alone.
    pub fn save(&mut self) -> io::Result<()> {
        let master = self
            .master
            .as_ref()
            .ok_or_else(|| io::Error::other("the keyring is locked"))?;
        self.index = self
            .contents
            .collections
            .iter()
            .map(|(name, collection)| {
                let items = collection
                    .items
                    .iter()
                    .map(|(id, item)| (*id, hashes(item)))
                    .collect();
                (name.clone(), items)
            })
            .collect();
        let plain = Zeroizing::new(toml::to_string(&self.contents).map_err(io::Error::other)?);
        let file = File {
            version: 1,
            cost: self.cost,
            salt: crypto::hex(&self.salt),
            master: crypto::hex(&self.sealed_master),
            contents: crypto::hex(&crypto::seal(master, plain.as_bytes())),
            index: self
                .index
                .iter()
                .map(|(name, items)| {
                    let items = items
                        .iter()
                        .map(|(id, hashes)| (id.to_string(), hashes.clone()))
                        .collect();
                    (name.clone(), items)
                })
                .collect(),
        };
        let text = toml::to_string(&file).map_err(io::Error::other)?;
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir)?;
        }
        let tmp = self.path.with_extension("tmp");
        {
            use std::os::unix::fs::OpenOptionsExt;
            let mut out = fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&tmp)?;
            io::Write::write_all(&mut out, text.as_bytes())?;
            out.sync_all()?;
        }
        fs::rename(tmp, &self.path)
    }

    /// The items (collection, id) whose attributes include all of `wanted`
    /// — found locked or not.
    pub fn search(
        &self,
        collection: Option<&str>,
        wanted: &BTreeMap<String, String>,
    ) -> Vec<(String, u64)> {
        let wanted: Vec<String> = wanted
            .iter()
            .map(|(name, value)| hash(name, value))
            .collect();
        self.index
            .iter()
            .filter(|(name, _)| collection.is_none_or(|only| only == name.as_str()))
            .flat_map(|(name, items)| {
                items
                    .iter()
                    .filter(|(_, hashes)| wanted.iter().all(|hash| hashes.contains(hash)))
                    .map(|(id, _)| (name.clone(), *id))
            })
            .collect()
    }

    /// Store `item` in `collection` — in place of one with the same
    /// attributes, if `replace`. Returns its id. Unlocked only.
    pub fn store(&mut self, collection: &str, item: Item, replace: bool) -> Result<u64, String> {
        if !self.is_unlocked() {
            return Err("locked".to_owned());
        }
        let existing = if replace {
            self.contents
                .collections
                .get(collection)
                .and_then(|found| {
                    found
                        .items
                        .iter()
                        .find(|(_, other)| other.attributes == item.attributes)
                })
                .map(|(id, _)| *id)
        } else {
            None
        };
        let id = existing.unwrap_or_else(|| {
            self.contents.next_id += 1;
            self.contents.next_id
        });
        let target = self
            .contents
            .collections
            .get_mut(collection)
            .ok_or_else(|| format!("no collection {collection}"))?;
        let created = target
            .items
            .get(&id)
            .map_or(item.created, |old| old.created);
        target.items.insert(id, Item { created, ..item });
        target.modified = now();
        self.save().map_err(|err| err.to_string())?;
        Ok(id)
    }
}

/// Seconds since 1970.
pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIGHT: Cost = Cost {
        memory: 1024,
        passes: 1,
    };

    fn item(service: &str, secret: &str) -> Item {
        Item {
            label: format!("Token for {service}"),
            attributes: [
                ("service".to_owned(), service.to_owned()),
                ("user".to_owned(), "ren".to_owned()),
            ]
            .into_iter()
            .collect(),
            secret: Zeroizing::new(secret.as_bytes().to_vec()),
            content_type: "text/plain".to_owned(),
            created: now(),
            modified: now(),
        }
    }

    #[test]
    fn kept_sealed_found_while_locked_and_rewrapped() {
        let dir = std::env::temp_dir().join(format!("enso-keyring-store-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let path = dir.join("keyring.toml");
        let mut keyring = Keyring::create(&path, b"old login", LIGHT).unwrap();
        let id = keyring
            .store("login", item("github.com", "gho_secret"), true)
            .unwrap();
        // The same attributes again replace it.
        assert_eq!(
            keyring
                .store("login", item("github.com", "gho_newer"), true)
                .unwrap(),
            id
        );
        keyring
            .store("login", item("gitlab.com", "glpat"), true)
            .unwrap();

        // On disk: no secret, no attribute, no label in sight.
        let text = fs::read_to_string(&path).unwrap();
        for plain in ["gho_newer", "github", "Token", "ren"] {
            assert!(!text.contains(plain), "{plain} is visible");
        }
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }

        // Locked, it can still be found — not read.
        let mut locked = Keyring::load(&path).unwrap().unwrap();
        let wanted: BTreeMap<String, String> = [("service".to_owned(), "github.com".to_owned())]
            .into_iter()
            .collect();
        assert_eq!(locked.search(None, &wanted), [("login".to_owned(), id)]);
        assert!(locked.contents.collections.is_empty());
        assert_eq!(locked.unlock(b"wrong"), Err(Refused::Password));
        locked.unlock(b"old login").unwrap();
        assert_eq!(
            locked.contents.collections["login"].items[&id]
                .secret
                .as_slice(),
            b"gho_newer"
        );

        // The login password changed: re-sealed, the old one no longer works.
        locked.rewrap(b"new login").unwrap();
        let mut again = Keyring::load(&path).unwrap().unwrap();
        assert_eq!(again.unlock(b"old login"), Err(Refused::Password));
        again.unlock(b"new login").unwrap();
        assert_eq!(again.contents.collections["login"].items.len(), 2);
        again.lock();
        assert!(!again.is_unlocked() && again.contents.collections.is_empty());
        fs::remove_dir_all(dir).unwrap();
    }
}
