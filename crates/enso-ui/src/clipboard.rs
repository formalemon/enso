//! The clipboard without a window: `ext-data-control`, the protocol
//! clipboard managers use.
//!
//! A window copies through its own surface (`Cx::copy`), which needs the
//! click or key that asked for it — and what it copies is gone when the
//! program quits, because the program itself serves every paste. [`hold`]
//! is the way round both: it puts something on the clipboard with no
//! surface at all, then stays to serve it until something else is copied.
//! The launcher copies an answer this way and lingers, unseen, as long as
//! the answer is on the clipboard.

use std::{io::Write, os::fd::OwnedFd, sync::Arc, thread};

use smithay_client_toolkit::reexports::{
    client::{
        Connection, Dispatch, Proxy, QueueHandle, event_created_child,
        globals::{GlobalListContents, registry_queue_init},
        protocol::{wl_registry::WlRegistry, wl_seat::WlSeat},
    },
    protocols::ext::data_control::v1::client::{
        ext_data_control_device_v1::{self, ExtDataControlDeviceV1},
        ext_data_control_manager_v1::ExtDataControlManagerV1,
        ext_data_control_offer_v1::ExtDataControlOfferV1,
        ext_data_control_source_v1::{self, ExtDataControlSourceV1},
    },
};

use crate::shell::Error;

/// Something to put on the clipboard: one thing, in each of its formats (a
/// MIME type and the bytes).
pub type Formats = Vec<(String, Vec<u8>)>;

struct Holder {
    formats: Arc<Formats>,
    /// Something else was copied: nothing left to serve.
    replaced: bool,
}

/// Put `formats` on the clipboard and serve it until something else is
/// copied. Blocks until then; returns at once with an error if the
/// compositor offers no data-control.
pub fn hold(formats: Formats) -> Result<(), Error> {
    let conn = Connection::connect_to_env()
        .map_err(|err| Error::new("cannot connect to a Wayland compositor", err))?;
    let (globals, mut queue) =
        registry_queue_init::<Holder>(&conn).map_err(|err| Error::new("Wayland registry", err))?;
    let qh = queue.handle();
    let seat: WlSeat = globals
        .bind(&qh, 1..=1, ())
        .map_err(|err| Error::new("compositor has no seat", err))?;
    let manager: ExtDataControlManagerV1 = globals
        .bind(&qh, 1..=1, ())
        .map_err(|err| Error::new("compositor lacks ext-data-control", err))?;
    let device = manager.get_data_device(&seat, &qh, ());
    let source = manager.create_data_source(&qh, ());
    for (mime, _) in &formats {
        source.offer(mime.clone());
    }
    device.set_selection(Some(&source));

    let mut holder = Holder {
        formats: Arc::new(formats),
        replaced: false,
    };
    while !holder.replaced {
        queue
            .blocking_dispatch(&mut holder)
            .map_err(|err| Error::new("Wayland connection", err))?;
    }
    source.destroy();
    device.destroy();
    Ok(())
}

/// Write one format to whoever pasted, off to the side: a slow reader never
/// holds up the next.
fn serve(formats: &Arc<Formats>, mime: &str, fd: OwnedFd) {
    let formats = Arc::clone(formats);
    let mime = mime.to_owned();
    let _ = thread::Builder::new()
        .name("enso-clip-send".to_owned())
        .spawn(move || {
            if let Some((_, bytes)) = formats.iter().find(|(offered, _)| *offered == mime) {
                let _ = std::fs::File::from(fd).write_all(bytes);
            }
        });
}

impl Dispatch<ExtDataControlSourceV1, ()> for Holder {
    fn event(
        holder: &mut Self,
        _: &ExtDataControlSourceV1,
        event: ext_data_control_source_v1::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            ext_data_control_source_v1::Event::Send { mime_type, fd } => {
                serve(&holder.formats, &mime_type, fd);
            }
            ext_data_control_source_v1::Event::Cancelled => holder.replaced = true,
            _ => {}
        }
    }
}

impl Dispatch<ExtDataControlDeviceV1, ()> for Holder {
    fn event(
        holder: &mut Self,
        _: &ExtDataControlDeviceV1,
        event: ext_data_control_device_v1::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let ext_data_control_device_v1::Event::Finished = event {
            holder.replaced = true;
        }
    }

    event_created_child!(Holder, ExtDataControlDeviceV1, [
        ext_data_control_device_v1::EVT_DATA_OFFER_OPCODE => (ExtDataControlOfferV1, ()),
    ]);
}

impl Dispatch<ExtDataControlOfferV1, ()> for Holder {
    fn event(
        _: &mut Self,
        offer: &ExtDataControlOfferV1,
        _: <ExtDataControlOfferV1 as Proxy>::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // Offers of what others copy are of no interest to a holder.
        let _ = offer;
    }
}

/// Objects whose events are of no interest here.
macro_rules! ignore {
    ($($object:ty, $data:ty);* $(;)?) => {$(
        impl Dispatch<$object, $data> for Holder {
            fn event(
                _: &mut Self,
                _: &$object,
                _: <$object as Proxy>::Event,
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
    ExtDataControlManagerV1, ();
}

/// Plain text, under every name programs look for it by.
pub fn text(text: &str) -> Formats {
    crate::Clip::text(text).formats
}
