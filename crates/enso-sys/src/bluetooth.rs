//! Bluetooth through BlueZ's D-Bus API (`org.bluez`).
//!
//! [`Bluez::state`] is a complete snapshot of the adapter and every device
//! it knows or can currently see; take it again after every change reported
//! by [`Bluez::watch`].
//!
//! Pairing a device that needs nothing typed or confirmed (headphones,
//! speakers, mice, most controllers) works on its own. One that wants a PIN,
//! a passkey, or numbers compared asks BlueZ's pairing agent — enso-auth,
//! if it's running, which shows the question on screen; without an agent
//! such a device can't be paired from here.

use crate::bus::{self, Error, Objects, Props, Result, Service};

const NAME: &str = "org.bluez";
const ADAPTER: &str = "org.bluez.Adapter1";
const DEVICE: &str = "org.bluez.Device1";
const BATTERY: &str = "org.bluez.Battery1";

/// A device the adapter knows (paired) or can currently see.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Device {
    /// Opaque handle for [`Bluez::connect_to`] and friends.
    pub id: String,
    /// The name to show: the alias the user gave it, else its own name.
    pub name: String,
    /// The hardware address, `AA:BB:CC:DD:EE:FF`.
    pub address: String,
    /// What BlueZ thinks it is, as a freedesktop icon name
    /// (`audio-headset`, `input-mouse`, …).
    pub kind: Option<String>,
    pub paired: bool,
    pub connected: bool,
    /// Battery level `0..=100`, for devices that report one.
    pub battery: Option<u8>,
}

/// Everything about Bluetooth at one moment.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Bluetooth {
    /// The radio is on.
    pub powered: bool,
    /// Looking for nearby devices.
    pub discovering: bool,
    /// Connected devices first, then paired, then merely nearby; by name
    /// within each group. Nearby devices that do not say what they are
    /// called are left out.
    pub devices: Vec<Device>,
}

impl Bluetooth {
    pub fn connected(&self) -> impl Iterator<Item = &Device> {
        self.devices.iter().filter(|device| device.connected)
    }
}

/// A connection to BlueZ.
#[derive(Clone, Debug)]
pub struct Bluez {
    service: Service,
}

impl Bluez {
    /// Connect to the system bus. BlueZ itself may be absent:
    /// [`Bluez::state`] then reports `None`.
    pub fn connect() -> Result<Self> {
        Service::connect(NAME).map(|service| Self { service })
    }

    /// The current state, or `None` when BlueZ is not running or there is no
    /// Bluetooth adapter.
    pub fn state(&self) -> Result<Option<Bluetooth>> {
        let objects = match self.service.objects() {
            Ok(objects) => objects,
            Err(err) if err.is_unavailable() => return Ok(None),
            Err(err) => return Err(err),
        };
        Ok(snapshot(&objects))
    }

    /// Turn the radio on or off.
    pub fn set_powered(&self, on: bool) -> Result<()> {
        let adapter = self.adapter()?;
        self.service.set(&adapter, ADAPTER, "Powered", on)
    }

    /// Start or stop looking for nearby devices. Discovery belongs to this
    /// connection: BlueZ ends it when the connection closes.
    pub fn set_discovering(&self, on: bool) -> Result<()> {
        let adapter = self.adapter()?;
        let method = if on {
            "StartDiscovery"
        } else {
            "StopDiscovery"
        };
        match self.service.run(&adapter, ADAPTER, method, &()) {
            // Already in the state asked for.
            Err(err) if err.is("InProgress") || err.is("NotReady") && !on => Ok(()),
            Err(err) if !on && err.is("Failed") => Ok(()),
            other => other,
        }
    }

    /// Connect to a device, pairing with it first if it is new. Waits until
    /// it is connected or has failed.
    pub fn connect_to(&self, device: &str) -> Result<()> {
        let objects = self.service.objects()?;
        let props = device_props(&objects, device)?;
        if !bus::prop(props, "Paired").unwrap_or(false) {
            match self.service.run(device, DEVICE, "Pair", &()) {
                Err(err) if !err.is("AlreadyExists") => return Err(err),
                _ => {}
            }
            // Trusted devices may reconnect on their own later.
            self.service.set(device, DEVICE, "Trusted", true)?;
        }
        match self.service.run(device, DEVICE, "Connect", &()) {
            Err(err) if err.is("AlreadyConnected") => Ok(()),
            other => other,
        }
    }

    /// Disconnect a device; it stays paired.
    pub fn disconnect(&self, device: &str) -> Result<()> {
        self.service.run(device, DEVICE, "Disconnect", &())
    }

    /// Forget a device entirely: unpair it and drop what is known about it.
    pub fn remove(&self, device: &str) -> Result<()> {
        let adapter = self.adapter()?;
        let path =
            zbus::wire::ObjectPath::try_from(device).map_err(|err| Error::new(err.to_string()))?;
        self.service
            .run(&adapter, ADAPTER, "RemoveDevice", &(path,))
    }

    /// Block, calling `on_change` whenever Bluetooth state may have changed,
    /// until it returns `false`. Take a new [`Bluez::state`] each time.
    pub fn watch(&self, on_change: impl FnMut() -> bool) -> Result<()> {
        self.service.watch(on_change)
    }

    /// Object path of the adapter.
    fn adapter(&self) -> Result<String> {
        let objects = self.service.objects()?;
        adapter(&objects)
            .map(|(path, _)| path.to_owned())
            .ok_or_else(|| Error::new("no Bluetooth adapter"))
    }
}

/// The first adapter, by object path (`hci0` before `hci1`).
fn adapter(objects: &Objects) -> Option<(&str, &Props)> {
    objects
        .iter()
        .filter_map(|(path, interfaces)| Some((path.as_str(), interfaces.get(ADAPTER)?)))
        .min_by_key(|(path, _)| *path)
}

fn device_props<'a>(objects: &'a Objects, device: &str) -> Result<&'a Props> {
    objects
        .iter()
        .find(|(path, _)| path.as_str() == device)
        .and_then(|(_, interfaces)| interfaces.get(DEVICE))
        .ok_or_else(|| Error::new("that device is no longer around"))
}

fn snapshot(objects: &Objects) -> Option<Bluetooth> {
    let (adapter_path, adapter) = adapter(objects)?;
    let mut devices: Vec<Device> = objects
        .iter()
        .filter_map(|(path, interfaces)| {
            let props = interfaces.get(DEVICE)?;
            if bus::path(props, "Adapter").as_deref() != Some(adapter_path) {
                return None;
            }
            let paired = bus::prop(props, "Paired").unwrap_or(false);
            let connected = bus::prop(props, "Connected").unwrap_or(false);
            // Without a name of its own, the alias is just the address: the
            // air is full of such beacons, and nobody means to pick one.
            let named = bus::string(props, "Name").is_some();
            if !(paired || connected || named) {
                return None;
            }
            let address = bus::string(props, "Address").unwrap_or_default();
            Some(Device {
                id: path.to_string(),
                name: bus::string(props, "Alias").unwrap_or_else(|| address.clone()),
                address,
                kind: bus::string(props, "Icon"),
                paired,
                connected,
                battery: interfaces
                    .get(BATTERY)
                    .and_then(|battery| bus::prop::<u8>(battery, "Percentage")),
            })
        })
        .collect();
    devices.sort_by(|a, b| {
        let rank = |device: &Device| (!device.connected, !device.paired);
        (rank(a), a.name.to_lowercase(), &a.id).cmp(&(rank(b), b.name.to_lowercase(), &b.id))
    });
    Some(Bluetooth {
        powered: bus::prop(adapter, "Powered").unwrap_or(false),
        discovering: bus::prop(adapter, "Discovering").unwrap_or(false),
        devices,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use zbus::wire::{ObjectPath, OwnedObjectPath, OwnedVariant, Variant};

    use super::*;

    fn owned<'a>(value: impl Into<Variant<'a>>) -> OwnedVariant {
        value.into().try_to_owned().unwrap()
    }

    fn object(
        path: &str,
        interface: &str,
        props: Vec<(&str, OwnedVariant)>,
    ) -> (OwnedObjectPath, HashMap<String, Props>) {
        let props: Props = props
            .into_iter()
            .map(|(name, value)| (name.to_owned(), value))
            .collect();
        (
            ObjectPath::try_from(path.to_owned()).unwrap().into(),
            HashMap::from([(interface.to_owned(), props)]),
        )
    }

    fn device(
        name: &str,
        alias: &str,
        named: bool,
        paired: bool,
        connected: bool,
    ) -> (OwnedObjectPath, HashMap<String, Props>) {
        let mut props = vec![
            (
                "Adapter",
                owned(ObjectPath::try_from("/org/bluez/hci0").unwrap()),
            ),
            ("Address", owned(format!("AA:BB:CC:00:00:{name}"))),
            ("Alias", owned(alias)),
            ("Paired", owned(paired)),
            ("Connected", owned(connected)),
        ];
        if named {
            props.push(("Name", owned(alias)));
        }
        object(&format!("/org/bluez/hci0/dev_{name}"), DEVICE, props)
    }

    #[test]
    fn no_adapter_means_no_bluetooth() {
        assert_eq!(snapshot(&Objects::new()), None);
    }

    #[test]
    fn devices_are_ordered_and_anonymous_beacons_dropped() {
        let mut objects: Objects = [
            object(
                "/org/bluez/hci0",
                ADAPTER,
                vec![("Powered", owned(true)), ("Discovering", owned(true))],
            ),
            device("01", "Zed Speaker", true, false, false),
            device("02", "Mouse", true, true, false),
            device("03", "Buds", true, true, true),
            // Seen during discovery, never said its name.
            device("04", "AA-BB-CC-00-00-04", false, false, false),
            device("05", "Alto Keyboard", true, true, false),
        ]
        .into_iter()
        .collect();
        // The connected one reports its battery.
        objects
            .iter_mut()
            .find(|(path, _)| path.as_str().ends_with("dev_03"))
            .unwrap()
            .1
            .insert(
                BATTERY.to_owned(),
                HashMap::from([("Percentage".to_owned(), owned(80u8))]),
            );

        let bluetooth = snapshot(&objects).unwrap();
        assert!(bluetooth.powered && bluetooth.discovering);
        let names: Vec<&str> = bluetooth.devices.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["Buds", "Alto Keyboard", "Mouse", "Zed Speaker"]);
        assert_eq!(bluetooth.devices[0].battery, Some(80));
        assert_eq!(bluetooth.devices[0].address, "AA:BB:CC:00:00:03");
        assert_eq!(bluetooth.connected().count(), 1);
    }
}
