//! The little of D-Bus the backends share: one system-bus connection, method
//! calls, properties, `GetManagedObjects`, and change notification.

use std::{
    collections::HashMap, fmt, future::poll_fn, pin::Pin, task::Poll, thread, time::Duration,
};

use zbus::{
    Connection, MatchRule, MessageStream,
    export::futures_core::Stream,
    message::Type,
    wire::{DynamicType, OwnedObjectPath, OwnedVariant, Variant},
};

/// Properties of one interface of one object.
pub(crate) type Props = HashMap<String, OwnedVariant>;
/// What `GetManagedObjects` returns: object path → interface → properties.
pub(crate) type Objects = HashMap<OwnedObjectPath, HashMap<String, Props>>;

/// A system service said no, or could not be reached.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    /// The D-Bus error name, when the service itself answered with an error
    /// (e.g. `net.connman.iwd.NoAgent`).
    name: Option<String>,
    message: String,
}

impl Error {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self {
            name: None,
            message: message.into(),
        }
    }

    /// The D-Bus error name, if the service answered with an error.
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    /// Whether the service answered with an error whose name ends in `suffix`
    /// (`"NoAgent"`, `"InProgress"`, …).
    pub(crate) fn is(&self, suffix: &str) -> bool {
        self.name
            .as_deref()
            .is_some_and(|name| name.rsplit('.').next() == Some(suffix))
    }

    /// The service is not running (and cannot be started on demand).
    pub fn is_unavailable(&self) -> bool {
        self.is("ServiceUnknown") || self.is("NameHasNoOwner")
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Error {}

impl From<zbus::Error> for Error {
    fn from(err: zbus::Error) -> Self {
        match &err {
            zbus::Error::MethodError(name, detail, _) => Self {
                name: Some(name.to_string()),
                // Services put the readable part in the detail; the name is
                // the fallback.
                message: detail
                    .clone()
                    .filter(|detail| !detail.is_empty())
                    .unwrap_or_else(|| name.to_string()),
            },
            zbus::Error::FDO(fdo) => Self {
                name: Some(format!("org.freedesktop.DBus.Error.{}", fdo_name(fdo))),
                message: fdo.to_string(),
            },
            other => Self::new(other.to_string()),
        }
    }
}

/// The variant name of a standard error, which is also its D-Bus name.
fn fdo_name(err: &zbus::fdo::Error) -> String {
    let debug = format!("{err:?}");
    debug
        .split(|c: char| !c.is_alphanumeric())
        .next()
        .unwrap_or_default()
        .to_owned()
}

pub(crate) type Result<T> = std::result::Result<T, Error>;

/// A connection to one service on the system bus.
#[derive(Clone, Debug)]
pub(crate) struct Service {
    conn: Connection,
    name: &'static str,
}

impl Service {
    /// Connect to the system bus. The service itself need not be running;
    /// calls to an absent service fail with [`Error::is_unavailable`].
    pub(crate) fn connect(name: &'static str) -> Result<Self> {
        let conn = zbus::block_on(Connection::system())
            .map_err(|err| Error::new(format!("cannot reach the system bus: {err}")))?;
        Ok(Self { conn, name })
    }

    pub(crate) fn connection(&self) -> &Connection {
        &self.conn
    }

    /// Call a method and decode its reply.
    pub(crate) fn call<B, R>(
        &self,
        path: &str,
        interface: &str,
        method: &str,
        body: &B,
    ) -> Result<R>
    where
        B: zbus::export::serde::Serialize + DynamicType,
        R: for<'d> zbus::export::serde::Deserialize<'d> + zbus::wire::Type,
    {
        zbus::block_on(async {
            let reply = self
                .conn
                .call_method(Some(self.name), path, Some(interface), method, body)
                .await?;
            Ok(reply.body().deserialize::<R>()?)
        })
    }

    /// Call a method that answers with nothing.
    pub(crate) fn run<B>(&self, path: &str, interface: &str, method: &str, body: &B) -> Result<()>
    where
        B: zbus::export::serde::Serialize + DynamicType,
    {
        self.call(path, interface, method, body)
    }

    /// Every object the service exports, with its properties.
    pub(crate) fn objects(&self) -> Result<Objects> {
        self.call(
            "/",
            "org.freedesktop.DBus.ObjectManager",
            "GetManagedObjects",
            &(),
        )
    }

    /// Set a property.
    pub(crate) fn set<'v>(
        &self,
        path: &str,
        interface: &str,
        property: &str,
        value: impl Into<Variant<'v>>,
    ) -> Result<()> {
        self.run(
            path,
            "org.freedesktop.DBus.Properties",
            "Set",
            &(interface, property, value.into()),
        )
    }

    /// Block, calling `on_change` after each burst of signals from the
    /// service (properties changing, objects coming and going) until it
    /// returns `false`. A burst is reported once, shortly after it starts:
    /// one scan result can be dozens of signals.
    pub(crate) fn watch(&self, mut on_change: impl FnMut() -> bool) -> Result<()> {
        const SETTLE: Duration = Duration::from_millis(60);
        let closed = || Error::new("the system bus connection closed");
        let mut signals = zbus::block_on(async {
            let rule = MatchRule::builder()
                .msg_type(Type::Signal)
                .sender(self.name)
                .build()?;
            MessageStream::for_match_rule(rule, &self.conn, None).await
        })?;
        loop {
            zbus::block_on(poll_fn(|cx| Pin::new(&mut signals).poll_next(cx)))
                .ok_or_else(closed)??;
            // Let the rest of the burst arrive, then set it aside unread.
            thread::sleep(SETTLE);
            zbus::block_on(poll_fn(|cx| {
                loop {
                    match Pin::new(&mut signals).poll_next(cx) {
                        Poll::Ready(Some(_)) => {}
                        Poll::Ready(None) => return Poll::Ready(Err(closed())),
                        Poll::Pending => return Poll::Ready(Ok(())),
                    }
                }
            }))?;
            if !on_change() {
                return Ok(());
            }
        }
    }
}

/// A property as `T`, if present and of that type.
pub(crate) fn prop<'a, T>(props: &'a Props, name: &str) -> Option<T>
where
    T: TryFrom<&'a OwnedVariant>,
{
    props.get(name).and_then(|value| T::try_from(value).ok())
}

/// A string property, owned.
pub(crate) fn string(props: &Props, name: &str) -> Option<String> {
    prop::<&str>(props, name).map(str::to_owned)
}

/// An object-path property, as a string.
pub(crate) fn path(props: &Props, name: &str) -> Option<String> {
    prop::<&zbus::wire::ObjectPath<'_>>(props, name).map(|path| path.to_string())
}
