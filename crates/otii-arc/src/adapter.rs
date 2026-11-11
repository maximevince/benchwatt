//! The Arc behind the device-neutral traits of `benchwatt-device`.
use crate::{
    device::{self, Arc},
    msg::{self, BlockBody},
    prop::{self, PropertyValue, ValueKind},
};
use benchwatt_device::{
    Block, ChannelDesc, Data, Device, DeviceInfo, Error, ErrorKind, Event, Gpio, Kind, Meter,
    OutputDesc, OutputState, Range, Result, SettingDesc, SettingKind, Severity, Supply, Uart,
    Value,
};
use std::sync::{
    Arc as StdArc, Mutex,
    mpsc::{self, Receiver, Sender},
};

impl From<device::Error> for Error {
    fn from(e: device::Error) -> Self {
        let kind = match e {
            device::Error::Timeout => ErrorKind::Timeout,
            device::Error::Property(_) => ErrorKind::Invalid,
            device::Error::Closed => ErrorKind::Closed,
            _ => ErrorKind::Io,
        };
        Error::new(kind, e.to_string())
    }
}

struct Stream {
    sink: Sender<Block>,
    first: Option<u32>,
}
#[derive(Default)]
struct Shared {
    stream: Mutex<Option<Stream>>,
    subscribers: Mutex<Vec<Sender<Event>>>,
    four_wire: Mutex<Option<bool>>,
}
impl Shared {
    fn on_event(&self, event: &device::Event) {
        let event = match event {
            device::Event::Data { ms, block } => {
                if let Some(s) = self.stream.lock().unwrap().as_mut() {
                    let first = *s.first.get_or_insert(*ms);
                    let _ = s.sink.send(convert(ms.wrapping_sub(first), block));
                }
                return;
            }
            device::Event::Gap { .. } => return,
            device::Event::Monitor { ch, value } => Event::Reading {
                ch: *ch as u16,
                value: *value,
            },
            device::Event::Gpi(mask) => Event::Inputs(*mask),
            device::Event::Property { .. } => Event::Changed,
            device::Event::FourWire(v) => {
                *self.four_wire.lock().unwrap() = Some(*v != 0);
                Event::Changed
            }
            device::Event::Alert { id, name } => {
                let (title, detail, severity) = alert_text(*id);
                Event::Alert {
                    id: *id,
                    name: (*name).into(),
                    title: title.into(),
                    detail: detail.into(),
                    severity,
                }
            }
            device::Event::Disconnected => {
                // Dropping the sink tells whoever reads the stream that it is over.
                *self.stream.lock().unwrap() = None;
                Event::Disconnected
            }
        };
        self.subscribers
            .lock()
            .unwrap()
            .retain(|s| s.send(event.clone()).is_ok());
    }
}

pub struct ArcDevice {
    arc: Arc,
    shared: StdArc<Shared>,
    gpo: Mutex<[Option<bool>; 2]>,
}

pub fn convert(ms: u32, b: &msg::Block) -> Block {
    let per = |n: usize| if n == 0 { 0.001 } else { 0.001 / n as f64 };
    let (dt, data) = match &b.body {
        BlockBody::Samples(v) => (
            per(v.len()),
            Data::Analog(v.iter().map(|&x| x as f32).collect()),
        ),
        BlockBody::Digital(v) => (per(v.len()), Data::Digital(v.clone())),
        BlockBody::Bytes(v) => (0.001, Data::Bytes(v.clone())),
    };
    Block {
        ch: b.ch,
        t: ms as f64 / 1000.0,
        dt,
        data,
    }
}

pub fn channels(chinfo: &prop::ChannelTable) -> Vec<ChannelDesc> {
    prop::CHANNELS
        .iter()
        .map(|&(id, name, unit)| {
            let kind = match id {
                0x16 => Kind::Digital,
                0x17 | 0x19 => Kind::Bytes,
                _ => Kind::Analog,
            };
            let (label, extra) = match name {
                "mc" => ("Current", false),
                "mv" => ("Voltage", false),
                "mp" => ("Power", false),
                "ac" => ("ADC current", false),
                "av" => ("ADC voltage", false),
                "ap" => ("ADC power", true),
                "sn" => ("Sense −", true),
                "sp" => ("Sense +", true),
                "vb" => ("VBUS", false),
                "vj" => ("DC jack", true),
                "tp" => ("Temperature", false),
                "gpi" => ("Digital inputs", false),
                "rx" => ("UART RX", false),
                "tx" => ("UART TX", true),
                _ => (name, true),
            };
            ChannelDesc {
                group: if matches!(name, "mc" | "mv" | "mp") {
                    "main".into()
                } else {
                    String::new()
                },
                // The Arc has two digital inputs; the other mask bits are not inputs.
                bits: if id == 0x16 { 2 } else { 0 },
                extra,
                ..ChannelDesc::new(id, name, label, unit, chinfo.get(id).rate_hz, kind)
            }
        })
        .collect()
}
/// The channels and blocks of captured traffic, one hex payload per line as in
/// `fixtures/session1.hex`, the way a meter stream would deliver them.
pub fn replay(capture: &str) -> (Vec<ChannelDesc>, Vec<Block>) {
    let mut table = prop::ChannelTable::default();
    let (mut first, mut blocks) = (None, Vec::new());
    for line in capture
        .lines()
        .filter(|s| !s.is_empty() && !s.starts_with('#'))
    {
        let bytes: Vec<u8> = line
            .as_bytes()
            .chunks_exact(2)
            .filter_map(|b| u8::from_str_radix(std::str::from_utf8(b).ok()?, 16).ok())
            .collect();
        match msg::parse(&bytes, &table) {
            // The reply to the channel table request in that capture.
            Ok(msg::Message::CmdResp { txn: 14, body, .. }) => {
                table = prop::ChannelTable::decode(&body)
            }
            Ok(msg::Message::Data { ms, blocks: data }) => {
                let first = *first.get_or_insert(ms);
                blocks.extend(data.iter().map(|b| convert(ms.wrapping_sub(first), b)));
            }
            _ => {}
        }
    }
    (channels(&table), blocks)
}
const MODES: [&str; 2] = ["cutoff", "limit"];
fn property_id(key: &str) -> Result<u32> {
    let id = match key.strip_prefix("0x") {
        Some(x) => u32::from_str_radix(x, 16).ok(),
        None => prop::property_by_name(key).map(|p| p.id),
    };
    id.ok_or_else(|| Error::invalid(format!("unknown property {key}")))
}
fn number(v: &PropertyValue) -> Result<f64> {
    match v {
        PropertyValue::Number(x) => Ok(*x),
        PropertyValue::Int(x) | PropertyValue::Enum(x) => Ok(*x as f64),
        PropertyValue::Bool(x) => Ok(f64::from(u8::from(*x))),
        _ => Err(Error::new(ErrorKind::Io, format!("unexpected value {v}"))),
    }
}
fn check_out(out: u8) -> Result<()> {
    if out == 1 {
        Ok(())
    } else {
        Err(Error::invalid(format!("no output {out}")))
    }
}

impl ArcDevice {
    pub fn open(path: &str) -> Result<Self> {
        Ok(Self::new(Arc::open(path)?))
    }
    pub fn new(arc: Arc) -> Self {
        let shared = StdArc::new(Shared::default());
        let listener = shared.clone();
        arc.listen(move |e| listener.on_event(e));
        Self {
            arc,
            shared,
            gpo: Mutex::new([None; 2]),
        }
    }
}

impl Device for ArcDevice {
    fn info(&self) -> DeviceInfo {
        let i = self.arc.info();
        let text = |id| {
            i.properties
                .get(&id)
                .map_or(String::new(), |v| v.to_string())
        };
        let mut props = serde_json::Map::new();
        for (id, v) in &i.properties {
            props.insert(
                prop::property(*id).map_or(format!("0x{id:x}"), |p| p.name.into()),
                Value::String(v.to_string()),
            );
        }
        let mut details = serde_json::Map::new();
        details.insert("protocol".into(), i.proto.into());
        details.insert("max_payload".into(), i.max_len.into());
        details.insert("device_type".into(), i.devtype.into());
        details.insert("features".into(), i.featurebits.into());
        details.insert("properties".into(), Value::Object(props));
        DeviceInfo {
            kind: "arc".into(),
            model: text(1),
            serial: text(0x82),
            firmware: text(0x81),
            hardware: text(0x80),
            details,
        }
    }
    fn channels(&self) -> Vec<ChannelDesc> {
        channels(&self.arc.info().chinfo)
    }
    fn settings(&self) -> Vec<SettingDesc> {
        let setting = |key: &str, label: &str, kind, output, readonly| SettingDesc {
            key: key.into(),
            label: label.into(),
            kind,
            output,
            readonly,
        };
        vec![
            setting(
                "current_mode",
                "Current mode",
                SettingKind::Choice {
                    options: MODES.iter().map(|m| (*m).into()).collect(),
                },
                Some(1),
                false,
            ),
            setting(
                "io_voltage",
                "Digital voltage",
                SettingKind::Number {
                    min: 1.2,
                    max: 5.0,
                    unit: "V".into(),
                },
                None,
                false,
            ),
            setting("5v_pin", "5 V pin", SettingKind::Bool, None, false),
            setting("4wire", "4-wire", SettingKind::Bool, None, true),
        ]
    }
    fn get(&self, key: &str) -> Result<Value> {
        match key {
            "current_mode" => {
                let mode = number(&self.arc.get_value(6)?)? as usize;
                return Ok(MODES.get(mode).map_or(Value::Null, |m| (*m).into()));
            }
            "io_voltage" => return self.get("expvolt"),
            "5v_pin" => return Ok((number(&self.arc.get_value(0x34)?)? > 0.0).into()),
            // What the device reports while measuring wins over the setting.
            "4wire" => {
                if let Some(active) = *self.shared.four_wire.lock().unwrap() {
                    return Ok(active.into());
                }
            }
            _ => {}
        }
        Ok(match self.arc.get_value(property_id(key)?)? {
            PropertyValue::String(s) => Value::String(s),
            PropertyValue::Bool(b) => Value::Bool(b),
            PropertyValue::Int(x) | PropertyValue::Enum(x) => x.into(),
            PropertyValue::Number(x) => x.into(),
            PropertyValue::Raw(b) => Value::String(format!("{b:x?}")),
        })
    }
    fn set(&self, key: &str, value: Value) -> Result<()> {
        match (key, &value) {
            ("current_mode", Value::String(m)) => {
                let mode = MODES
                    .iter()
                    .position(|x| x == m)
                    .ok_or_else(|| Error::invalid("mode must be cutoff or limit"))?;
                return self.set("mainsrccurrmode", mode.into());
            }
            ("io_voltage", _) => return self.set("expvolt", value),
            ("5v_pin", Value::Bool(on)) => {
                return self.set("expoutvolt", if *on { 5.0 } else { 0.0 }.into());
            }
            ("current_mode" | "5v_pin", _) => {
                return Err(Error::invalid(format!("wrong value type for {key}")));
            }
            _ => {}
        }
        let id = property_id(key)?;
        let Some(p) = prop::property(id) else {
            // Not in the table: the driver refuses it with its own error.
            return Ok(self.arc.set(id, &PropertyValue::Raw(Vec::new()))?);
        };
        let wrong = || -> Error {
            device::Error::Property(format!("wrong value type for {}", p.name)).into()
        };
        let v = match (p.kind, &value) {
            (ValueKind::String, Value::String(s)) => PropertyValue::String(s.clone()),
            (ValueKind::String, v) => PropertyValue::String(v.to_string()),
            (ValueKind::Bool, Value::Bool(b)) => PropertyValue::Bool(*b),
            (ValueKind::Bool, Value::Number(n)) => {
                PropertyValue::Bool(n.as_f64().ok_or_else(wrong)? != 0.0)
            }
            (ValueKind::Int, Value::Number(n)) => {
                PropertyValue::Int(n.as_f64().ok_or_else(wrong)? as i32)
            }
            (ValueKind::Enum, Value::Number(n)) => {
                PropertyValue::Enum(n.as_f64().ok_or_else(wrong)? as i32)
            }
            (ValueKind::Scaled, Value::Number(n)) => {
                PropertyValue::Number(n.as_f64().ok_or_else(wrong)?)
            }
            _ => return Err(wrong()),
        };
        self.arc.set(id, &v)?;
        if matches!(id, 0x33 | 0x34) {
            self.arc.commit()?;
        }
        Ok(())
    }
    fn events(&self) -> Receiver<Event> {
        let (tx, rx) = mpsc::channel();
        self.shared.subscribers.lock().unwrap().push(tx);
        rx
    }
    fn supply(&self) -> Option<&dyn Supply> {
        Some(self)
    }
    fn meter(&self) -> Option<&dyn Meter> {
        Some(self)
    }
    fn gpio(&self) -> Option<&dyn Gpio> {
        Some(self)
    }
    fn uart(&self) -> Option<&dyn Uart> {
        Some(self)
    }
}

impl Supply for ArcDevice {
    fn outputs(&self) -> Vec<OutputDesc> {
        let max = self
            .arc
            .get_value(0x80b)
            .ok()
            .or_else(|| self.arc.info().properties.get(&0x80b).cloned())
            .and_then(|v| match v {
                PropertyValue::Number(x) => Some(x),
                _ => None,
            })
            .unwrap_or(5.0);
        vec![OutputDesc {
            n: 1,
            label: "Main output".into(),
            group: "main".into(),
            voltage: Range { min: 0.0, max },
            voltage_steps: Vec::new(),
            current_limit: Some(Range {
                min: 0.001,
                max: 3.0,
            }),
        }]
    }
    fn set_voltage(&self, out: u8, volts: f64) -> Result<()> {
        check_out(out)?;
        Ok(self.arc.set_voltage(volts)?)
    }
    fn set_current_limit(&self, out: u8, amps: f64) -> Result<()> {
        check_out(out)?;
        Ok(self.arc.set_current_limit(amps)?)
    }
    fn set_enabled(&self, out: u8, on: bool) -> Result<()> {
        check_out(out)?;
        Ok(self.arc.output(on)?)
    }
    fn state(&self, out: u8) -> Result<OutputState> {
        check_out(out)?;
        Ok(OutputState {
            enabled: number(&self.arc.get_value(9)?)? != 0.0,
            voltage: Some(number(&self.arc.get_value(11)?)?),
            current_limit: Some(number(&self.arc.get_value(12)?)?),
        })
    }
}

impl Meter for ArcDevice {
    fn start(&self, channels: &[u16], sink: Sender<Block>) -> Result<()> {
        *self.shared.stream.lock().unwrap() = Some(Stream { sink, first: None });
        let ids: Vec<u32> = channels.iter().map(|&c| c.into()).collect();
        let result = self.arc.start_recording(&ids);
        if result.is_err() {
            *self.shared.stream.lock().unwrap() = None;
        }
        Ok(result?)
    }
    fn stop(&self) -> Result<()> {
        let result = self.arc.stop_recording();
        *self.shared.stream.lock().unwrap() = None;
        Ok(result?)
    }
}

impl Gpio for ArcDevice {
    fn outputs(&self) -> Vec<Option<bool>> {
        self.gpo.lock().unwrap().to_vec()
    }
    fn set_output(&self, n: u8, on: bool) -> Result<()> {
        self.arc.set_gpo(n, on)?;
        if let Some(level) = self.gpo.lock().unwrap().get_mut(usize::from(n) - 1) {
            *level = Some(on);
        }
        Ok(())
    }
}

impl Uart for ArcDevice {
    fn open(&self, baud: u32) -> Result<()> {
        Ok(self.arc.uart_open(baud)?)
    }
    fn close(&self) -> Result<()> {
        Ok(self.arc.uart_close()?)
    }
    fn write(&self, bytes: &[u8]) -> Result<()> {
        Ok(self.arc.uart_write(bytes)?)
    }
    fn rx(&self) -> u16 {
        0x17
    }
    fn tx(&self) -> u16 {
        0x19
    }
}

/// How the app words an alert the Arc sends by number.
fn alert_text(id: u16) -> (&'static str, &'static str, Severity) {
    use Severity::{Error, Warning};
    match id {
        1 => (
            "Overtemperature",
            "Device temperature exceeded the safe limit.",
            Error,
        ),
        2 => (
            "VBUS undervoltage",
            "VBUS voltage is below the expected level.",
            Warning,
        ),
        3 => (
            "DC input overvoltage",
            "DC input voltage exceeded the safe limit.",
            Error,
        ),
        4 => (
            "Voltage calibration failed",
            "Voltage calibration could not be completed.",
            Error,
        ),
        5 => (
            "Output overvoltage",
            "Output voltage exceeded the configured limit.",
            Error,
        ),
        6 => ("Overcurrent", "Output switched off (cutoff mode).", Error),
        7 => (
            "Load switch overload",
            "The load switch exceeded its safe operating limit.",
            Error,
        ),
        8 => (
            "DC/DC overload",
            "The DC/DC converter exceeded its safe operating limit.",
            Error,
        ),
        9 => (
            "Pre-regulator overload",
            "The pre-regulator exceeded its safe operating limit.",
            Error,
        ),
        10 => (
            "Sink overload",
            "The sink exceeded its safe operating limit.",
            Error,
        ),
        11 => (
            "Negative internal resistance",
            "The device detected negative internal resistance.",
            Error,
        ),
        12 => (
            "Bad firmware",
            "The device firmware is invalid or incompatible.",
            Error,
        ),
        _ => (
            "Device alert",
            "The device reported an alert this app has no description for.",
            Error,
        ),
    }
}
