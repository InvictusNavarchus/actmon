use std::{
    fs::{self, File},
    io,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use anyhow::{Context, Result, bail};
use evdev::{EventType, InputEvent, KeyCode, RelativeAxisCode, raw_stream::RawDevice};
use serde::Serialize;

use crate::{
    clock,
    model::{Action, BUTTON, KEYBOARD, MOTION, WHEEL},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Keyboard,
    Mouse,
}

#[derive(Clone, Debug, Serialize)]
pub struct DeviceInfo {
    pub node: PathBuf,
    pub name: String,
    pub sysfs: PathBuf,
}

/// Metadata discovery works even when the caller cannot open device nodes.
pub fn discover(sysfs: &Path, dev_root: &Path) -> Result<Vec<DeviceInfo>> {
    let mut devices = Vec::new();
    for entry in fs::read_dir(sysfs).context("enumerate input metadata")? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name
            .strip_prefix("event")
            .is_some_and(|tail| !tail.is_empty() && tail.bytes().all(|b| b.is_ascii_digit()))
        {
            continue;
        }
        let path = entry.path();
        // read_to_string preserves deliberate trailing spaces in device names.
        let Ok(device_name) = fs::read_to_string(path.join("device/name")) else {
            continue;
        };
        devices.push(DeviceInfo {
            node: dev_root.join(name.as_ref()),
            name: device_name.trim_end_matches('\n').to_owned(),
            sysfs: fs::canonicalize(path)?,
        });
    }
    devices.sort_by(|a, b| a.node.cmp(&b.node));
    Ok(devices)
}

pub fn select<'a>(devices: &'a [DeviceInfo], name: &str) -> Result<Option<&'a DeviceInfo>> {
    let matches: Vec<_> = devices.iter().filter(|d| d.name == name).collect();
    if matches.len() > 1 {
        bail!("multiple devices named {name:?}; refusing ambiguous selection");
    }
    Ok(matches.first().copied())
}

fn has_pointer_buttons(keys: &evdev::AttributeSetRef<KeyCode>) -> bool {
    keys.iter()
        .any(|key| (KeyCode::BTN_LEFT.code()..=KeyCode::BTN_TASK.code()).contains(&key.code()))
}

pub struct Reader {
    pub info: DeviceInfo,
    device: RawDevice,
    pub decoder: Decoder,
    pub mask: u8,
    pub repeats_supported: bool,
}

impl Reader {
    pub fn open(info: DeviceInfo, role: Role) -> Result<Self> {
        // RawDevice::open attempts write access first. Open explicitly read-only.
        let file = File::open(&info.node)
            .with_context(|| format!("open {} read-only", info.node.display()))?;
        rustix::io::ioctl_fionbio(&file, true)?;
        let device = RawDevice::from_fd(file.into())?;
        clock::set_input_clock(&device).context("configure evdev CLOCK_MONOTONIC")?;
        let keys = device.supported_keys();
        let axes = device.supported_relative_axes();
        let has_axis = |code| axes.is_some_and(|axes| axes.contains(code));
        let mask = match role {
            Role::Keyboard => {
                if !keys.is_some_and(|keys| {
                    keys.contains(KeyCode::KEY_A) && keys.contains(KeyCode::KEY_ENTER)
                }) {
                    bail!("selected keyboard does not expose keyboard capabilities");
                }
                KEYBOARD
            }
            Role::Mouse => {
                if !has_axis(RelativeAxisCode::REL_X) || !has_axis(RelativeAxisCode::REL_Y) {
                    bail!("selected mouse must expose relative X and Y axes");
                }
                MOTION
                    | if keys.is_some_and(has_pointer_buttons) {
                        BUTTON
                    } else {
                        0
                    }
                    | if [
                        RelativeAxisCode::REL_WHEEL,
                        RelativeAxisCode::REL_HWHEEL,
                        RelativeAxisCode::REL_WHEEL_HI_RES,
                        RelativeAxisCode::REL_HWHEEL_HI_RES,
                    ]
                    .into_iter()
                    .any(has_axis)
                    {
                        WHEEL
                    } else {
                        0
                    }
            }
        };
        let decoder = Decoder::new(
            role,
            has_axis(RelativeAxisCode::REL_WHEEL_HI_RES),
            has_axis(RelativeAxisCode::REL_HWHEEL_HI_RES),
        );
        let repeats_supported =
            role == Role::Keyboard && device.supported_events().contains(EventType::REPEAT);
        Ok(Self {
            info,
            device,
            decoder,
            mask,
            repeats_supported,
        })
    }

    pub fn fd(&self) -> &RawDevice {
        &self.device
    }

    pub fn read(&mut self) -> io::Result<Vec<InputEvent>> {
        self.device.fetch_events().map(|events| events.collect())
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Decoded {
    Action(Action),
    Dropped,
    Recovered,
    Ignore,
}

/// Raw mode deliberately avoids synthesized state-recovery transitions. Counts
/// do not require held-key state; after SYN_DROPPED resume after SYN_REPORT.
pub struct Decoder {
    role: Role,
    hires: [bool; 2],
    dropping: bool,
}

impl Decoder {
    pub fn new(role: Role, vertical_hires: bool, horizontal_hires: bool) -> Self {
        Self {
            role,
            hires: [vertical_hires, horizontal_hires],
            dropping: false,
        }
    }

    pub fn decode(&mut self, event: &InputEvent) -> Decoded {
        let (ty, code, value) = (event.event_type().0, event.code(), event.value());
        if ty == 0 && code == 3 {
            self.dropping = true;
            return Decoded::Dropped;
        }
        if self.dropping {
            if ty == 0 && code == 0 {
                self.dropping = false;
                return Decoded::Recovered;
            }
            return Decoded::Ignore;
        }
        let action = match (self.role, ty, code, value) {
            // Exclude BTN_* ranges; include extended keyboard keys starting 0x160.
            (Role::Keyboard, 1, 1..=255 | 352..=543 | 560..=703, 1) => Some(Action::Key),
            (Role::Keyboard, 1, 1..=255 | 352..=543 | 560..=703, 2) => Some(Action::Repeat),
            (Role::Mouse, 1, 272..=279, 1) => Some(Action::Button(usize::from(code - 272))),
            (Role::Mouse, 2, 0..=1, v) if v != 0 => Some(Action::Motion),
            (Role::Mouse, 2, 8, v) if v != 0 && !self.hires[0] => Some(Action::Wheel {
                axis: 0,
                amount: i64::from(v) * 120,
            }),
            (Role::Mouse, 2, 6, v) if v != 0 && !self.hires[1] => Some(Action::Wheel {
                axis: 1,
                amount: i64::from(v) * 120,
            }),
            (Role::Mouse, 2, 11, v) if v != 0 && self.hires[0] => Some(Action::Wheel {
                axis: 0,
                amount: i64::from(v),
            }),
            (Role::Mouse, 2, 12, v) if v != 0 && self.hires[1] => Some(Action::Wheel {
                axis: 1,
                amount: i64::from(v),
            }),
            _ => None,
        };
        action.map_or(Decoded::Ignore, Decoded::Action)
    }
}

pub fn event_time(event: &InputEvent) -> Result<i64> {
    Ok(i64::try_from(
        event.timestamp().duration_since(UNIX_EPOCH)?.as_micros(),
    )?)
}

#[cfg(test)]
mod tests;
