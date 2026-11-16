//! Knowing when nobody is at the machine, through `ext-idle-notify`.
//!
//! The compositor is the one that sees every key and every pointer move, so
//! it is the one asked: tell me when there has been none for this long. It
//! does not count time during which a program — a film, a call — has asked
//! for the screen to stay awake.

use std::time::Duration;

use smithay_client_toolkit::reexports::{
    client::{
        Connection, Dispatch, QueueHandle,
        globals::{GlobalListContents, registry_queue_init},
        protocol::{wl_registry::WlRegistry, wl_seat::WlSeat},
    },
    protocols::ext::idle_notify::v1::client::{
        ext_idle_notification_v1::{self, ExtIdleNotificationV1},
        ext_idle_notifier_v1::ExtIdleNotifierV1,
    },
};

use crate::shell::Error;

struct Watcher<F> {
    on_idle: F,
}

/// Block, calling `on_idle` each time `after` has passed without input.
/// It is called once per stretch of idleness: not again until somebody has
/// come back and left again. Returns only if the compositor goes away, or
/// cannot do this.
pub fn watch_idle(after: Duration, on_idle: impl FnMut() + 'static) -> Result<(), Error> {
    let conn = Connection::connect_to_env()
        .map_err(|err| Error::new("cannot connect to a Wayland compositor", err))?;
    let (globals, mut queue) = registry_queue_init::<Watcher<_>>(&conn)
        .map_err(|err| Error::new("Wayland registry", err))?;
    let qh = queue.handle();
    let seat: WlSeat = globals
        .bind(&qh, 1..=1, ())
        .map_err(|err| Error::new("compositor has no seat", err))?;
    let notifier: ExtIdleNotifierV1 = globals
        .bind(&qh, 1..=1, ())
        .map_err(|err| Error::new("compositor lacks ext-idle-notify", err))?;
    let millis = u32::try_from(after.as_millis()).unwrap_or(u32::MAX);
    let _notification = notifier.get_idle_notification(millis, &seat, &qh, ());

    let mut watcher = Watcher { on_idle };
    loop {
        queue
            .blocking_dispatch(&mut watcher)
            .map_err(|err| Error::new("Wayland connection", err))?;
    }
}

impl<F: FnMut() + 'static> Dispatch<ExtIdleNotificationV1, ()> for Watcher<F> {
    fn event(
        watcher: &mut Self,
        _: &ExtIdleNotificationV1,
        event: ext_idle_notification_v1::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if matches!(event, ext_idle_notification_v1::Event::Idled) {
            (watcher.on_idle)();
        }
    }
}

/// Objects whose events are of no interest here.
macro_rules! ignore {
    ($($object:ty, $data:ty);* $(;)?) => {$(
        impl<F: FnMut() + 'static> Dispatch<$object, $data> for Watcher<F> {
            fn event(
                _: &mut Self,
                _: &$object,
                _: <$object as smithay_client_toolkit::reexports::client::Proxy>::Event,
                _: &$data,
                _: &Connection,
                _: &QueueHandle<Self>,
            ) {
            }
        }
    )*};
}

ignore! {
    WlRegistry, GlobalListContents;
    WlSeat, ();
    ExtIdleNotifierV1, ();
}
