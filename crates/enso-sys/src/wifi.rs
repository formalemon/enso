//! Wi-Fi through iwd's D-Bus API (`net.connman.iwd`).
//!
//! [`Iwd::state`] is a complete snapshot — radio, connection, and the
//! networks in range — cheap enough to take again after every change, which
//! is what [`Iwd::watch`] is for.

use zbus::{interface, wire::OwnedObjectPath};

use crate::bus::{self, Error, Objects, Props, Result, Service};

const NAME: &str = "net.connman.iwd";
const DEVICE: &str = "net.connman.iwd.Device";
const STATION: &str = "net.connman.iwd.Station";
const NETWORK: &str = "net.connman.iwd.Network";
const KNOWN_NETWORK: &str = "net.connman.iwd.KnownNetwork";
const AGENT_MANAGER: &str = "net.connman.iwd.AgentManager";
const ROOT: &str = "/net/connman/iwd";
/// Where the passphrase agent is exported while a connection is attempted.
const AGENT_PATH: &str = "/dev/enso/WifiAgent";

/// How a network is secured.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Security {
    Open,
    /// WPA/WPA2/WPA3 personal: a passphrase.
    Passphrase,
    /// WPA enterprise (802.1X): needs a profile iwd has been given.
    Enterprise,
    /// WEP, which iwd does not connect to.
    Wep,
}

/// What the radio is doing about being connected.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Link {
    #[default]
    Disconnected,
    Connecting,
    Connected,
    Disconnecting,
}

/// A network in range.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Network {
    /// Opaque handle for [`Iwd::connect`] and [`Iwd::forget`].
    pub id: String,
    /// The SSID.
    pub name: String,
    pub security: Security,
    /// Signal strength in dBm (about -30 excellent, -90 unusable).
    pub signal_dbm: i16,
    pub connected: bool,
    /// iwd has credentials for it, so connecting needs nothing more.
    pub known: bool,
}

impl Network {
    /// Signal strength as bars, `0..=4`.
    pub fn bars(&self) -> u8 {
        match self.signal_dbm {
            -55.. => 4,
            -66.. => 3,
            -77.. => 2,
            -88.. => 1,
            _ => 0,
        }
    }
}

/// Everything about Wi-Fi at one moment.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Wifi {
    /// Kernel name of the wireless interface, e.g. `wlan0`.
    pub interface: String,
    /// The radio is on.
    pub powered: bool,
    pub link: Link,
    /// A scan is in progress.
    pub scanning: bool,
    /// Networks in range, strongest first, the connected one before all.
    pub networks: Vec<Network>,
}

impl Wifi {
    /// The network in use, if any.
    pub fn connected(&self) -> Option<&Network> {
        self.networks.iter().find(|network| network.connected)
    }
}

/// Why connecting did not work.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnectError {
    /// The network wants a passphrase and none was given.
    NeedsPassphrase,
    /// The passphrase that was given did not get us in.
    Rejected,
    Other(Error),
}

impl std::fmt::Display for ConnectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NeedsPassphrase => f.write_str("the network needs a passphrase"),
            Self::Rejected => f.write_str("the passphrase was not accepted"),
            Self::Other(err) => err.fmt(f),
        }
    }
}

impl std::error::Error for ConnectError {}

/// A connection to iwd.
#[derive(Clone, Debug)]
pub struct Iwd {
    service: Service,
}

impl Iwd {
    /// Connect to the system bus. iwd itself may be absent: [`Iwd::state`]
    /// then reports `None`.
    pub fn connect() -> Result<Self> {
        Service::connect(NAME).map(|service| Self { service })
    }

    /// The current state, or `None` when iwd is not running or there is no
    /// wireless device.
    pub fn state(&self) -> Result<Option<Wifi>> {
        let objects = match self.service.objects() {
            Ok(objects) => objects,
            Err(err) if err.is_unavailable() => return Ok(None),
            Err(err) => return Err(err),
        };
        let Some((device, props)) = device(&objects) else {
            return Ok(None);
        };
        let mut wifi = Wifi {
            interface: bus::string(props, "Name").unwrap_or_default(),
            powered: bus::prop(props, "Powered").unwrap_or(false),
            ..Wifi::default()
        };
        // A powered-off device has no station, and so no networks.
        let Some(station) = interface(&objects, device, STATION) else {
            return Ok(Some(wifi));
        };
        wifi.scanning = bus::prop(station, "Scanning").unwrap_or(false);
        wifi.link = match bus::string(station, "State").as_deref() {
            Some("connected" | "roaming") => Link::Connected,
            Some("connecting") => Link::Connecting,
            Some("disconnecting") => Link::Disconnecting,
            _ => Link::Disconnected,
        };

        // iwd orders these the way a person would: connected, then known,
        // then by signal.
        let ordered: Vec<(OwnedObjectPath, i16)> =
            self.service
                .call(device, STATION, "GetOrderedNetworks", &())?;
        wifi.networks = ordered
            .into_iter()
            .filter_map(|(path, signal)| {
                let props = interface(&objects, path.as_str(), NETWORK)?;
                Some(Network {
                    id: path.to_string(),
                    name: bus::string(props, "Name")?,
                    security: match bus::string(props, "Type")?.as_str() {
                        "open" => Security::Open,
                        "psk" => Security::Passphrase,
                        "wep" => Security::Wep,
                        _ => Security::Enterprise,
                    },
                    // Reported in hundredths of a dBm.
                    signal_dbm: signal / 100,
                    connected: bus::prop(props, "Connected").unwrap_or(false),
                    known: bus::path(props, "KnownNetwork").is_some(),
                })
            })
            .collect();
        Ok(Some(wifi))
    }

    /// Turn the radio on or off.
    pub fn set_powered(&self, on: bool) -> Result<()> {
        let device = self.device()?;
        self.service.set(&device, DEVICE, "Powered", on)
    }

    /// Look for networks. Returns at once; results arrive as changes. A scan
    /// already under way is not an error.
    pub fn scan(&self) -> Result<()> {
        let device = self.device()?;
        match self.service.run(&device, STATION, "Scan", &()) {
            Err(err) if err.is_busy() => Ok(()),
            other => other,
        }
    }

    /// Connect to a network from [`Wifi::networks`], waiting until it is up
    /// or has failed. `passphrase` is used if iwd asks for one; a known
    /// network needs none.
    pub fn connect_to(
        &self,
        network: &str,
        passphrase: Option<&str>,
    ) -> std::result::Result<(), ConnectError> {
        // iwd asks whoever called `Connect` for the passphrase, through an
        // agent object that exists only for the length of this call.
        let agent = match passphrase {
            Some(passphrase) => Some(
                self.register_agent(passphrase)
                    .map_err(ConnectError::Other)?,
            ),
            None => None,
        };
        let result: Result<()> = self.service.run(network, NETWORK, "Connect", &());
        if agent.is_some() {
            self.unregister_agent();
        }
        result.map_err(|err| match passphrase {
            None if err.is("NoAgent") => ConnectError::NeedsPassphrase,
            // iwd reports a failed handshake as plain `Failed`; with a new
            // passphrase in play that is by far the likeliest reason.
            Some(_) if err.is("Failed") || err.is("InvalidFormat") => ConnectError::Rejected,
            _ => ConnectError::Other(err),
        })
    }

    /// Drop the current connection. iwd will not reconnect on its own until
    /// asked to connect again.
    pub fn disconnect(&self) -> Result<()> {
        let device = self.device()?;
        self.service.run(&device, STATION, "Disconnect", &())
    }

    /// Forget a known network's credentials.
    pub fn forget(&self, network: &str) -> Result<()> {
        let objects = self.service.objects()?;
        let known = interface(&objects, network, NETWORK)
            .and_then(|props| bus::path(props, "KnownNetwork"))
            .ok_or_else(|| Error::new("that network is not a saved one"))?;
        self.service.run(&known, KNOWN_NETWORK, "Forget", &())
    }

    /// Block, calling `on_change` whenever Wi-Fi state may have changed,
    /// until it returns `false`. Take a new [`Iwd::state`] each time.
    pub fn watch(&self, on_change: impl FnMut() -> bool) -> Result<()> {
        self.service.watch(on_change)
    }

    /// Object path of the wireless device.
    fn device(&self) -> Result<String> {
        let objects = self.service.objects()?;
        device(&objects)
            .map(|(path, _)| path.to_owned())
            .ok_or_else(|| Error::new("no wireless device"))
    }

    fn register_agent(&self, passphrase: &str) -> Result<()> {
        let agent = Agent {
            passphrase: passphrase.to_owned(),
        };
        let server = self.service.connection().object_server();
        zbus::block_on(async {
            // A previous attempt may have left one behind.
            let _ = server.remove::<Agent, _>(AGENT_PATH).await;
            server.at(AGENT_PATH, agent).await
        })
        .map_err(Error::from)?;
        let path = zbus::wire::ObjectPath::try_from(AGENT_PATH)
            .map_err(|err| Error::new(err.to_string()))?;
        match self
            .service
            .run(ROOT, AGENT_MANAGER, "RegisterAgent", &(path,))
        {
            Err(err) if !err.is("AlreadyExists") => Err(err),
            _ => Ok(()),
        }
    }

    fn unregister_agent(&self) {
        // Best effort: iwd also drops the agent when this connection closes.
        if let Ok(path) = zbus::wire::ObjectPath::try_from(AGENT_PATH) {
            let _ = self
                .service
                .run(ROOT, AGENT_MANAGER, "UnregisterAgent", &(path,));
        }
        let server = self.service.connection().object_server();
        let _ = zbus::block_on(server.remove::<Agent, _>(AGENT_PATH));
    }
}

impl Error {
    fn is_busy(&self) -> bool {
        self.is("Busy") || self.is("InProgress")
    }
}

/// The first device in station mode, or failing that any device (a
/// powered-off radio has no mode to speak of).
fn device(objects: &Objects) -> Option<(&str, &Props)> {
    let mut devices: Vec<(&str, &Props)> = objects
        .iter()
        .filter_map(|(path, interfaces)| Some((path.as_str(), interfaces.get(DEVICE)?)))
        .collect();
    devices.sort_by_key(|(path, props)| {
        let station = bus::string(props, "Mode").as_deref() == Some("station");
        (!station, *path)
    });
    devices.into_iter().next()
}

fn interface<'a>(objects: &'a Objects, path: &str, interface: &str) -> Option<&'a Props> {
    objects
        .iter()
        .find(|(candidate, _)| candidate.as_str() == path)
        .and_then(|(_, interfaces)| interfaces.get(interface))
}

/// Answers iwd's request for a passphrase during [`Iwd::connect_to`].
struct Agent {
    passphrase: String,
}

/// The one error an iwd agent may answer with.
#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "net.connman.iwd.Agent.Error")]
enum AgentError {
    #[zbus(error)]
    ZBus(zbus::Error),
    Canceled(String),
}

#[interface(name = "net.connman.iwd.Agent")]
impl Agent {
    fn request_passphrase(&self, _network: OwnedObjectPath) -> String {
        self.passphrase.clone()
    }

    // Enterprise credentials are provisioned as iwd profiles, not typed here.
    fn request_private_key_passphrase(
        &self,
        _network: OwnedObjectPath,
    ) -> std::result::Result<String, AgentError> {
        Err(AgentError::Canceled("not supported".to_owned()))
    }

    fn request_user_name_and_password(
        &self,
        _network: OwnedObjectPath,
    ) -> std::result::Result<(String, String), AgentError> {
        Err(AgentError::Canceled("not supported".to_owned()))
    }

    fn request_user_password(
        &self,
        _network: OwnedObjectPath,
        _user: String,
    ) -> std::result::Result<String, AgentError> {
        Err(AgentError::Canceled("not supported".to_owned()))
    }

    fn cancel(&self, _reason: String) {}

    fn release(&self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    fn network(signal_dbm: i16) -> Network {
        Network {
            id: "/net/connman/iwd/0/5/x_psk".into(),
            name: "x".into(),
            security: Security::Passphrase,
            signal_dbm,
            connected: false,
            known: false,
        }
    }

    #[test]
    fn signal_strength_becomes_bars() {
        let bars: Vec<u8> = [-40, -55, -56, -66, -70, -77, -80, -88, -89, -100]
            .into_iter()
            .map(|dbm| network(dbm).bars())
            .collect();
        assert_eq!(bars, [4, 4, 3, 3, 2, 2, 1, 1, 0, 0]);
    }

    #[test]
    fn the_connected_network_is_found() {
        let mut wifi = Wifi {
            networks: vec![network(-50), network(-60)],
            ..Wifi::default()
        };
        assert_eq!(wifi.connected(), None);
        wifi.networks[1].connected = true;
        assert_eq!(wifi.connected().map(|n| n.signal_dbm), Some(-60));
    }
}
