//! What an instrument looks like to the rest of Benchwatt: channel descriptions,
//! sample blocks and the capability traits a driver implements.
use serde::{Deserialize, Serialize};
use std::{
    fmt,
    sync::mpsc::{Receiver, Sender},
};

pub use serde_json::Value;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum Kind {
    Analog,
    Digital,
    Bytes,
}
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum Quantity {
    Current,
    Voltage,
    Power,
    Temperature,
    #[default]
    Other,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ChannelDesc {
    /// Unique within the device; carries no meaning outside the driver.
    pub id: u16,
    pub name: String,
    /// What a person calls it, e.g. `Current`.
    #[serde(default)]
    pub label: String,
    pub unit: String,
    /// Nominal sample rate; 0 for channels that only report events.
    pub rate_hz: u32,
    pub kind: Kind,
    #[serde(default)]
    pub quantity: Quantity,
    /// Channels measured at the same point share a group, so charge and energy
    /// know which current, voltage and power belong together. Empty for none.
    #[serde(default)]
    pub group: String,
    /// Digital channels: how many low bits of the mask are real inputs.
    #[serde(default)]
    pub bits: u8,
    /// Rarely needed; a channel list shows it last or hides it.
    #[serde(default)]
    pub extra: bool,
    /// Reports the value that was set, not one that was measured.
    #[serde(default)]
    pub set: bool,
}
impl ChannelDesc {
    pub fn new(id: u16, name: &str, label: &str, unit: &str, rate_hz: u32, kind: Kind) -> Self {
        Self {
            id,
            name: name.into(),
            label: label.into(),
            unit: unit.into(),
            rate_hz,
            kind,
            quantity: match unit {
                "A" => Quantity::Current,
                "V" => Quantity::Voltage,
                "W" => Quantity::Power,
                "°C" => Quantity::Temperature,
                _ => Quantity::Other,
            },
            group: String::new(),
            bits: 0,
            extra: false,
            set: false,
        }
    }
}
#[derive(Clone, Debug, PartialEq)]
pub enum Data {
    Analog(Vec<f32>),
    /// One input mask per sample.
    Digital(Vec<u32>),
    Bytes(Vec<u8>),
}
/// Samples of one channel. `t` is in seconds on the device's own clock and is 0
/// for the first block after `Meter::start`; `dt` is the sample period, or for
/// bytes the time the block covers.
#[derive(Clone, Debug, PartialEq)]
pub struct Block {
    pub ch: u16,
    pub t: f64,
    pub dt: f64,
    pub data: Data,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct DeviceInfo {
    /// The driver's name, e.g. `arc`.
    pub kind: String,
    pub model: String,
    pub serial: String,
    pub firmware: String,
    pub hardware: String,
    /// Whatever else the device reports about itself.
    pub details: serde_json::Map<String, Value>,
}
#[derive(Clone, Copy, Debug, Serialize, PartialEq)]
pub struct Range {
    pub min: f64,
    pub max: f64,
}
impl Range {
    pub fn contains(&self, x: f64) -> bool {
        x.is_finite() && x >= self.min && x <= self.max
    }
}
#[derive(Clone, Debug, Serialize)]
pub struct OutputDesc {
    pub n: u8,
    pub label: String,
    /// The group of the channels that measure this output.
    pub group: String,
    pub voltage: Range,
    /// Not empty: the only voltages the output can take.
    pub voltage_steps: Vec<f64>,
    /// `None` when the limit cannot be set.
    pub current_limit: Option<Range>,
}
/// A device-specific knob. Its value is a bool, one of the choices as a string,
/// or a number, by `kind`.
#[derive(Clone, Debug, Serialize)]
pub struct SettingDesc {
    pub key: String,
    pub label: String,
    #[serde(flatten)]
    pub kind: SettingKind,
    /// The output it belongs to, if any.
    pub output: Option<u8>,
    pub readonly: bool,
}
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum SettingKind {
    Bool,
    Choice { options: Vec<String> },
    Number { min: f64, max: f64, unit: String },
}
/// Setpoints, not measurements.
#[derive(Clone, Copy, Debug, Serialize, PartialEq)]
pub struct OutputState {
    pub enabled: bool,
    pub voltage: f64,
    pub current_limit: f64,
}
#[derive(Clone, Debug)]
pub enum Event {
    Disconnected,
    /// A live value of a channel, sent whether or not the meter is running.
    Reading {
        ch: u16,
        value: f32,
    },
    /// The digital input mask changed.
    Inputs(u32),
    /// A setting or output state changed on the device; read it again.
    Changed,
    Alert {
        id: u16,
        name: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    Io,
    Timeout,
    /// A value or name the device or driver does not accept.
    Invalid,
    Closed,
}
#[derive(Clone, Debug)]
pub struct Error {
    pub kind: ErrorKind,
    pub message: String,
}
impl Error {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Invalid, message)
    }
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for Error {}
impl Serialize for Error {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.message)
    }
}
pub type Result<T> = std::result::Result<T, Error>;

pub trait Device: Send + Sync {
    fn info(&self) -> DeviceInfo;
    fn channels(&self) -> Vec<ChannelDesc>;
    /// The device-specific values a user interface should offer. Anything two
    /// devices share in meaning belongs in a trait below instead.
    fn settings(&self) -> Vec<SettingDesc> {
        Vec::new()
    }
    /// Read or write a setting, or another value the driver knows by key.
    fn get(&self, key: &str) -> Result<Value>;
    fn set(&self, key: &str, value: Value) -> Result<()>;
    fn events(&self) -> Receiver<Event>;

    fn supply(&self) -> Option<&dyn Supply> {
        None
    }
    fn meter(&self) -> Option<&dyn Meter> {
        None
    }
    fn gpio(&self) -> Option<&dyn Gpio> {
        None
    }
    fn uart(&self) -> Option<&dyn Uart> {
        None
    }
}
/// Outputs are numbered from 1.
pub trait Supply {
    fn outputs(&self) -> Vec<OutputDesc>;
    fn set_voltage(&self, out: u8, volts: f64) -> Result<()>;
    fn set_current_limit(&self, out: u8, amps: f64) -> Result<()>;
    fn set_enabled(&self, out: u8, on: bool) -> Result<()>;
    fn state(&self, out: u8) -> Result<OutputState>;
}
pub trait Meter {
    /// Start streaming the given channels into `sink` until `stop`.
    fn start(&self, channels: &[u16], sink: Sender<Block>) -> Result<()>;
    fn stop(&self) -> Result<()>;
}
/// Outputs are numbered from 1. Input levels arrive as `Event::Inputs` and on
/// the device's digital channel.
pub trait Gpio {
    /// Last level set on each output; `None` where the device cannot tell.
    fn outputs(&self) -> Vec<Option<bool>>;
    fn set_output(&self, n: u8, on: bool) -> Result<()>;
}
pub trait Uart {
    fn open(&self, baud: u32) -> Result<()>;
    fn close(&self) -> Result<()>;
    fn write(&self, bytes: &[u8]) -> Result<()>;
    /// The bytes channel that received data arrives on.
    fn rx(&self) -> u16;
    /// The bytes channel a recording logs written data on.
    fn tx(&self) -> u16;
}
