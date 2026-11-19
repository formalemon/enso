//! Warnings when the battery runs low, sent to this daemon's own toasts.
//!
//! Nothing else on the desktop says it: the bar colours the battery, but
//! nobody looks at the bar while working. So the daemon looks at `/sys`
//! itself every half minute, and says so twice on the way down — at 10 %,
//! and at 5 % as critical, which shows even in Do not disturb and stays
//! until the machine is plugged in (or it's dismissed), with a button to
//! sleep now. Plugging in takes either warning away.

use std::{thread, time::Duration};

use enso_sys::power::{Battery, Sysfs};

use crate::{
    local::Local,
    stack::{Action, Notification, Timeout, Urgency},
};

/// How often the battery is looked at.
const EVERY: Duration = Duration::from_secs(30);
/// Below this, a warning.
const LOW: u8 = 10;
/// Below this, a critical one.
const CRITICAL: u8 = 5;
/// The ids the warnings go by: far above any the bus hands out.
pub const LOW_ID: u32 = u32::MAX - 1;
pub const CRITICAL_ID: u32 = u32::MAX - 2;

/// What the watcher has to say.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Say {
    Warn(Notification),
    /// Withdraw a warning: the machine was plugged in.
    Withdraw(u32),
}

/// How far down the warnings have gone.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Watch {
    low: bool,
    critical: bool,
}

impl Watch {
    /// What to say about `battery`, now.
    pub fn step(&mut self, battery: Option<&Battery>) -> Vec<Say> {
        let Some(battery) = battery else {
            return Vec::new();
        };
        let mut said = Vec::new();
        if battery.is_plugged_in() || battery.percent > LOW + 2 {
            // Back on power, or charged past the line: warn again next time.
            if self.critical && battery.is_plugged_in() {
                said.push(Say::Withdraw(CRITICAL_ID));
            }
            if self.low && battery.is_plugged_in() {
                said.push(Say::Withdraw(LOW_ID));
            }
            *self = Self::default();
            return said;
        }
        if battery.percent <= CRITICAL && !self.critical {
            self.critical = true;
            self.low = true;
            said.push(Say::Warn(warning(battery, true)));
        } else if battery.percent <= LOW && !self.low {
            self.low = true;
            said.push(Say::Warn(warning(battery, false)));
        }
        said
    }
}

fn warning(battery: &Battery, critical: bool) -> Notification {
    let left = match battery.minutes_left {
        Some(minutes) if minutes >= 60 => {
            format!("about {} h {} min left", minutes / 60, minutes % 60)
        }
        Some(minutes) => format!("about {minutes} min left"),
        None => "plug in soon".to_owned(),
    };
    let (summary, body, actions) = if critical {
        (
            format!("Battery very low — {}%", battery.percent),
            format!("Plug in now: {left}. Unsaved work goes with it if it runs out."),
            vec![Action {
                key: Local::Sleep.key(),
                label: "Sleep now".to_owned(),
            }],
        )
    } else {
        (
            format!("Battery low — {}%", battery.percent),
            format!("{}{}.", left[..1].to_uppercase(), &left[1..]),
            Vec::new(),
        )
    };
    Notification {
        id: if critical { CRITICAL_ID } else { LOW_ID },
        app_name: "Battery".to_owned(),
        summary,
        body,
        urgency: if critical {
            Urgency::Critical
        } else {
            Urgency::Normal
        },
        timeout: if critical {
            Timeout::Never
        } else {
            Timeout::After(Duration::from_secs(10))
        },
        actions,
        image: None,
        progress: None,
        transient: true,
    }
}

/// Look at the battery every half minute on a background thread, passing
/// on what there is to say; stops when `say` returns `false`. A machine
/// without a battery is never looked at again.
pub fn watch(mut say: impl FnMut(Say) -> bool + Send + 'static) {
    let spawned = thread::Builder::new()
        .name("enso-battery".to_owned())
        .spawn(move || {
            let sysfs = Sysfs::system();
            if sysfs.battery().is_none() {
                return;
            }
            let mut watch = Watch::default();
            loop {
                for said in watch.step(sysfs.battery().as_ref()) {
                    if !say(said) {
                        return;
                    }
                }
                thread::sleep(EVERY);
            }
        });
    if let Err(err) = spawned {
        eprintln!("enso-notify: cannot watch the battery: {err}");
    }
}

#[cfg(test)]
mod tests {
    use enso_sys::power::BatteryStatus;

    use super::*;

    fn battery(percent: u8, status: BatteryStatus) -> Battery {
        Battery {
            name: "BAT1".into(),
            percent,
            status,
            power_mw: None,
            minutes_left: Some(24),
            limit: None,
        }
    }

    fn ids(said: &[Say]) -> Vec<(bool, u32)> {
        said.iter()
            .map(|say| match say {
                Say::Warn(note) => (true, note.id),
                Say::Withdraw(id) => (false, *id),
            })
            .collect()
    }

    #[test]
    fn warns_twice_on_the_way_down_and_takes_it_back_on_power() {
        use BatteryStatus::*;
        let mut watch = Watch::default();
        assert!(watch.step(Some(&battery(40, Discharging))).is_empty());
        assert_eq!(
            ids(&watch.step(Some(&battery(10, Discharging)))),
            [(true, LOW_ID)]
        );
        // Once is enough.
        assert!(watch.step(Some(&battery(9, Discharging))).is_empty());
        let critical = watch.step(Some(&battery(5, Discharging)));
        assert_eq!(ids(&critical), [(true, CRITICAL_ID)]);
        let Say::Warn(note) = &critical[0] else {
            unreachable!()
        };
        assert_eq!(note.urgency, Urgency::Critical);
        assert_eq!(note.summary, "Battery very low — 5%");
        assert!(note.body.contains("about 24 min left"));
        assert_eq!(note.actions[0].label, "Sleep now");

        // Plugged in: both go, and the next run down warns again.
        assert_eq!(
            ids(&watch.step(Some(&battery(5, Charging)))),
            [(false, CRITICAL_ID), (false, LOW_ID)]
        );
        assert_eq!(
            ids(&watch.step(Some(&battery(9, Discharging)))),
            [(true, LOW_ID)]
        );
        // Straight to critical from above warns once, critically.
        let mut fresh = Watch::default();
        assert_eq!(
            ids(&fresh.step(Some(&battery(4, Discharging)))),
            [(true, CRITICAL_ID)]
        );
        assert!(fresh.step(Some(&battery(3, Discharging))).is_empty());
        // No battery, nothing to say.
        assert!(fresh.step(None).is_empty());
    }
}
