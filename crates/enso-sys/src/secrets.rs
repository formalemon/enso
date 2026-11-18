//! Keeping a secret in the session's keyring, through the Secret Service
//! (`org.freedesktop.secrets` — enso-keyring, or gnome-keyring, or any):
//! find one by its attributes, keep one, forget one. A locked keyring is
//! unlocked the spec's way — the keyring shows its own prompt — and this
//! waits for it.
//!
//! Secrets travel in a "plain" session, as libsecret's do by default.

use std::collections::HashMap;

use zbus::export::futures_core::Stream;
use zbus::{Connection, ObjectPath, OwnedObjectPath, OwnedVariant, Variant};

use crate::bus::{Error, Result};

const NAME: &str = "org.freedesktop.secrets";
const ROOT: &str = "/org/freedesktop/secrets";
const SERVICE: &str = "org.freedesktop.Secret.Service";

/// `(session, parameters, value, content type)`.
type Secret = (OwnedObjectPath, Vec<u8>, Vec<u8>, String);

async fn connect() -> zbus::Result<(Connection, OwnedObjectPath)> {
    let connection = Connection::session().await?;
    let reply = connection
        .call_method(
            Some(NAME),
            ROOT,
            Some(SERVICE),
            "OpenSession",
            &("plain", Variant::from("")),
        )
        .await?;
    let (_, session): (OwnedVariant, OwnedObjectPath) = reply.body().deserialize()?;
    Ok((connection, session))
}

/// Unlock `objects`, showing the keyring's prompt if it wants one.
/// Returns whether they're unlocked now.
async fn unlock(connection: &Connection, objects: &[OwnedObjectPath]) -> zbus::Result<bool> {
    let reply = connection
        .call_method(
            Some(NAME),
            ROOT,
            Some(SERVICE),
            "Unlock",
            &(objects.to_vec(),),
        )
        .await?;
    let (unlocked, prompt): (Vec<OwnedObjectPath>, OwnedObjectPath) = reply.body().deserialize()?;
    if prompt.as_str() == "/" {
        return Ok(!unlocked.is_empty() || objects.is_empty());
    }
    // Hear how it went before asking for it, so the answer isn't missed.
    let rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .interface("org.freedesktop.Secret.Prompt")
        .member("Completed")
        .path(prompt.as_str())
        .build()?;
    let mut completed = zbus::MessageStream::for_match_rule(rule, connection, None).await?;
    connection
        .call_method(
            Some(NAME),
            prompt.as_str(),
            Some("org.freedesktop.Secret.Prompt"),
            "Prompt",
            &("",),
        )
        .await?;
    let next = std::future::poll_fn(|cx| std::pin::Pin::new(&mut completed).poll_next(cx));
    if let Some(message) = next.await {
        let message = message?;
        let (dismissed, _): (bool, OwnedVariant) = message.body().deserialize()?;
        return Ok(!dismissed);
    }
    Ok(false)
}

/// The secret kept under these attributes, if there is one (unlocking the
/// keyring if need be — `Ok(None)` too if that's declined).
pub fn lookup(attributes: &HashMap<&str, &str>) -> Result<Option<Vec<u8>>> {
    zbus::block_on(async {
        let (connection, session) = connect().await?;
        let reply = connection
            .call_method(
                Some(NAME),
                ROOT,
                Some(SERVICE),
                "SearchItems",
                &(attributes,),
            )
            .await?;
        let (unlocked, locked): (Vec<OwnedObjectPath>, Vec<OwnedObjectPath>) =
            reply.body().deserialize()?;
        let items = if !unlocked.is_empty() {
            unlocked
        } else if !locked.is_empty() && unlock(&connection, &locked).await? {
            locked
        } else {
            return Ok(None);
        };
        let reply = connection
            .call_method(
                Some(NAME),
                ROOT,
                Some(SERVICE),
                "GetSecrets",
                &(items.clone(), session),
            )
            .await?;
        let secrets: HashMap<OwnedObjectPath, Secret> = reply.body().deserialize()?;
        zbus::Result::Ok(
            items
                .iter()
                .find_map(|item| secrets.get(item))
                .map(|(_, _, value, _)| value.clone()),
        )
    })
    .map_err(Error::from)
}

/// Keep `secret` under these attributes, in place of any kept under the
/// same ones; `label` is what keyring managers show.
pub fn store(label: &str, attributes: &HashMap<&str, &str>, secret: &[u8]) -> Result<()> {
    zbus::block_on(async {
        let (connection, session) = connect().await?;
        let reply = connection
            .call_method(Some(NAME), ROOT, Some(SERVICE), "ReadAlias", &("default",))
            .await?;
        let collection: OwnedObjectPath = reply.body().deserialize()?;
        if collection.as_str() == "/" {
            return Err(zbus::Error::Failure(
                "the keyring has no default collection".to_owned(),
            ));
        }
        if !unlock(&connection, std::slice::from_ref(&collection)).await? {
            return Err(zbus::Error::Failure("the keyring stayed locked".to_owned()));
        }
        let attributes: HashMap<String, String> = attributes
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect();
        let mut properties: HashMap<&str, Variant<'_>> = HashMap::new();
        properties.insert("org.freedesktop.Secret.Item.Label", Variant::from(label));
        properties.insert(
            "org.freedesktop.Secret.Item.Attributes",
            Variant::from(attributes),
        );
        let secret: (ObjectPath<'_>, Vec<u8>, Vec<u8>, &str) =
            (session.as_ref(), Vec::new(), secret.to_vec(), "text/plain");
        connection
            .call_method(
                Some(NAME),
                collection.as_str(),
                Some("org.freedesktop.Secret.Collection"),
                "CreateItem",
                &(properties, secret, true),
            )
            .await
            .map(drop)
    })
    .map_err(Error::from)
}

/// Forget every secret kept under these attributes.
pub fn clear(attributes: &HashMap<&str, &str>) -> Result<()> {
    zbus::block_on(async {
        let connection = Connection::session().await?;
        let reply = connection
            .call_method(
                Some(NAME),
                ROOT,
                Some(SERVICE),
                "SearchItems",
                &(attributes,),
            )
            .await?;
        let (unlocked, locked): (Vec<OwnedObjectPath>, Vec<OwnedObjectPath>) =
            reply.body().deserialize()?;
        let mut items = unlocked;
        if !locked.is_empty() && unlock(&connection, &locked).await? {
            items.extend(locked);
        }
        for item in items {
            connection
                .call_method(
                    Some(NAME),
                    item.as_str(),
                    Some("org.freedesktop.Secret.Item"),
                    "Delete",
                    &(),
                )
                .await?;
        }
        zbus::Result::Ok(())
    })
    .map_err(Error::from)
}
