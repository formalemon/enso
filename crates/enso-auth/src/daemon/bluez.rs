//! BlueZ's pairing agent (`org.bluez.Agent1`).
//!
//! Pairing a keyboard, a phone, or anything else that wants a code goes
//! through the default agent: it shows the code to type on the device, asks
//! for one the device shows, or asks whether the numbers on both screens
//! match. This one asks with the same prompt as for passwords, so pairing
//! from enso-ctl — or from anything else — works for every kind of device.

use std::{thread, time::Duration};

use zbus::{Connection, OwnedObjectPath, OwnedVariant, interface, message::Header};

use super::{Prompts, sent_by};
use crate::request::{Answer, Request};

pub const NAME: &str = "org.bluez";
pub const PATH: &str = "/dev/enso/BluetoothAgent";
/// Bluetooth prompts are taken back together: BlueZ cancels "the" request.
const TAG: &str = "bluetooth";
/// How long a code to type stays up, if pairing neither ends nor fails.
const SHOWN_FOR: Duration = Duration::from_secs(90);

pub struct Agent {
    pub prompts: Prompts,
}

#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.bluez.Error")]
pub enum Error {
    #[zbus(error)]
    ZBus(zbus::Error),
    Rejected(String),
    Canceled(String),
}

impl Agent {
    async fn ask(
        &self,
        connection: &Connection,
        header: &Header<'_>,
        request: impl FnOnce(String) -> Request,
        device: &OwnedObjectPath,
    ) -> Result<Answer, Error> {
        if !sent_by(connection, header, NAME).await {
            return Err(Error::Rejected("only BlueZ may ask".to_owned()));
        }
        let name = device_name(connection, device).await;
        Ok(self.prompts.ask(TAG, request(name)).await)
    }

    /// Show a code to type, until the device is paired (or a while passes).
    async fn show(
        &self,
        connection: &Connection,
        header: &Header<'_>,
        device: &OwnedObjectPath,
        code: String,
    ) -> Result<(), Error> {
        if !sent_by(connection, header, NAME).await {
            return Err(Error::Rejected("only BlueZ may ask".to_owned()));
        }
        let name = device_name(connection, device).await;
        // Not waited for: BlueZ goes on pairing while the code is up.
        drop(self.prompts.ask(TAG, Request::Show { device: name, code }));
        let (prompts, connection, device) =
            (self.prompts.clone(), connection.clone(), device.clone());
        thread::spawn(move || {
            let mut waited = Duration::ZERO;
            while waited < SHOWN_FOR {
                thread::sleep(Duration::from_millis(500));
                waited += Duration::from_millis(500);
                if zbus::block_on(paired(&connection, &device)) {
                    break;
                }
            }
            prompts.cancel(TAG);
        });
        Ok(())
    }
}

fn yes(answer: Answer) -> Result<(), Error> {
    match answer {
        Answer::Allow => Ok(()),
        _ => Err(Error::Rejected("declined".to_owned())),
    }
}

#[interface(name = "org.bluez.Agent1")]
impl Agent {
    fn release(&self) {}

    async fn request_pin_code(
        &self,
        device: OwnedObjectPath,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
    ) -> Result<String, Error> {
        match self
            .ask(
                connection,
                &header,
                |device| Request::Pin { device },
                &device,
            )
            .await?
        {
            Answer::Pin(pin) => Ok(pin),
            _ => Err(Error::Canceled("no PIN".to_owned())),
        }
    }

    async fn display_pin_code(
        &self,
        device: OwnedObjectPath,
        pincode: String,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
    ) -> Result<(), Error> {
        self.show(connection, &header, &device, pincode).await
    }

    async fn request_passkey(
        &self,
        device: OwnedObjectPath,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
    ) -> Result<u32, Error> {
        match self
            .ask(
                connection,
                &header,
                |device| Request::Passkey { device },
                &device,
            )
            .await?
        {
            Answer::Passkey(passkey) => Ok(passkey),
            _ => Err(Error::Canceled("no passkey".to_owned())),
        }
    }

    async fn display_passkey(
        &self,
        device: OwnedObjectPath,
        passkey: u32,
        entered: u16,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
    ) -> Result<(), Error> {
        // Called again as each digit is typed on the device; once is enough.
        if entered > 0 {
            return Ok(());
        }
        self.show(connection, &header, &device, format!("{passkey:06}"))
            .await
    }

    async fn request_confirmation(
        &self,
        device: OwnedObjectPath,
        passkey: u32,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
    ) -> Result<(), Error> {
        yes(self
            .ask(
                connection,
                &header,
                |device| Request::Confirm { device, passkey },
                &device,
            )
            .await?)
    }

    async fn request_authorization(
        &self,
        device: OwnedObjectPath,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
    ) -> Result<(), Error> {
        yes(self
            .ask(
                connection,
                &header,
                |device| Request::Authorize { device },
                &device,
            )
            .await?)
    }

    async fn authorize_service(
        &self,
        device: OwnedObjectPath,
        uuid: String,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
    ) -> Result<(), Error> {
        let service = service_name(&uuid).to_owned();
        yes(self
            .ask(
                connection,
                &header,
                |device| Request::Service { device, service },
                &device,
            )
            .await?)
    }

    fn cancel(&self) {
        self.prompts.cancel(TAG);
    }
}

/// Become BlueZ's default agent, able to show codes and take them.
pub async fn register(connection: &Connection) -> zbus::Result<()> {
    let path = zbus::ObjectPath::try_from(PATH)?;
    let manager = |method: &'static str, body| {
        let connection = connection.clone();
        async move {
            connection
                .call_method(
                    Some(NAME),
                    "/org/bluez",
                    Some("org.bluez.AgentManager1"),
                    method,
                    &body,
                )
                .await
        }
    };
    match manager("RegisterAgent", (path.clone(), "KeyboardDisplay")).await {
        Err(zbus::Error::MethodError(name, ..)) if name.as_str().ends_with("AlreadyExists") => {}
        other => drop(other?),
    }
    connection
        .call_method(
            Some(NAME),
            "/org/bluez",
            Some("org.bluez.AgentManager1"),
            "RequestDefaultAgent",
            &(path,),
        )
        .await
        .map(drop)
}

async fn property(
    connection: &Connection,
    device: &OwnedObjectPath,
    name: &str,
) -> Option<OwnedVariant> {
    let reply = connection
        .call_method(
            Some(NAME),
            device.as_str(),
            Some("org.freedesktop.DBus.Properties"),
            "Get",
            &("org.bluez.Device1", name),
        )
        .await
        .ok()?;
    reply.body().deserialize().ok()
}

/// What the device is called: the name given to it, else its own.
async fn device_name(connection: &Connection, device: &OwnedObjectPath) -> String {
    match property(connection, device, "Alias").await {
        Some(alias) => String::try_from(alias).unwrap_or_else(|_| "A device".to_owned()),
        None => "A device".to_owned(),
    }
}

async fn paired(connection: &Connection, device: &OwnedObjectPath) -> bool {
    match property(connection, device, "Paired").await {
        Some(paired) => bool::try_from(paired).unwrap_or(false),
        // Gone: nothing to wait for.
        None => true,
    }
}

/// A service's everyday name, from its UUID.
fn service_name(uuid: &str) -> &'static str {
    let short = uuid
        .strip_prefix("0000")
        .and_then(|rest| rest.get(..4))
        .unwrap_or("")
        .to_ascii_lowercase();
    match short.as_str() {
        "110a" | "110b" | "110d" | "110c" | "110e" | "110f" => "audio",
        "1108" | "1112" | "111e" | "111f" => "calls",
        "1124" | "1812" => "input",
        "1105" | "1106" => "file sharing",
        "1115" | "1116" | "1117" => "the network",
        "112f" | "1132" | "1133" | "1134" => "the phone book and messages",
        "1101" => "a serial port",
        "180f" => "its battery level",
        _ => "a service",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn services_have_everyday_names() {
        assert_eq!(
            service_name("0000110b-0000-1000-8000-00805f9b34fb"),
            "audio"
        );
        assert_eq!(
            service_name("00001124-0000-1000-8000-00805F9B34FB"),
            "input"
        );
        assert_eq!(
            service_name("12345678-0000-1000-8000-00805f9b34fb"),
            "a service"
        );
    }
}
