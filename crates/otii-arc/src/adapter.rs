//! The Arc behind the device-neutral traits of `benchwatt-device`.
use crate::{
    device::{self, Arc},
    msg::{self, BlockBody},
    prop::{self, PropertyValue, ValueKind},
};
use benchwatt_device::{
    Block, ChannelDesc, Data, Device, DeviceInfo, Error, ErrorKind, Event, Gpio, Kind, Meter,
    OutputDesc, OutputState, Quantity, Range, Result, Supply, Uart, Value,
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
            device::Event::Alert { id, name } => Event::Alert {
                id: *id,
                name: (*name).into(),
            },
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
        .map(|&(id, name, unit)| ChannelDesc {
            id,
            name: name.into(),
            unit: unit.into(),
            rate_hz: chinfo.get(id).rate_hz,
            kind: match id {
                0x16 => Kind::Digital,
                0x17 | 0x19 => Kind::Bytes,
                _ => Kind::Analog,
            },
            quantity: match unit {
                "A" => Quantity::Current,
                "V" => Quantity::Voltage,
                "W" => Quantity::Power,
                "°C" => Quantity::Temperature,
                _ => Quantity::Other,
            },
            group: if matches!(name, "mc" | "mv" | "mp") {
                "main".into()
            } else {
                String::new()
            },
            // The Arc has two digital inputs; the other mask bits are not inputs.
            bits: if id == 0x16 { 2 } else { 0 },
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
    fn get(&self, key: &str) -> Result<Value> {
        if key == "4wire_active" {
            return Ok(self
                .shared
                .four_wire
                .lock()
                .unwrap()
                .map_or(Value::Null, Value::Bool));
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
            voltage: Range { min: 0.0, max },
            current_limit: Range {
                min: 0.001,
                max: 3.0,
            },
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
            voltage: number(&self.arc.get_value(11)?)?,
            current_limit: number(&self.arc.get_value(12)?)?,
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
