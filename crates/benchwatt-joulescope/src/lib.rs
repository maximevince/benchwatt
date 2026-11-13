//! Joulescope JS220 behind the device-neutral traits of `benchwatt-device`,
//! on top of Jetperch's own driver.
mod jsdrv;

use benchwatt_device::{
    Block, ChannelDesc, Data, Device, DeviceInfo, Error, Event, Gpio, Kind, Meter, Result,
    SettingDesc, SettingKind, Value,
};
use jsdrv::{Driver, Message, Subscription};
use std::{
    ffi::c_void,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread,
    time::Duration,
};

pub const CH_I: u16 = 0;
pub const CH_V: u16 = 1;
pub const CH_P: u16 = 2;
/// Name, the topic that switches its stream on, and the topic the data comes on.
const STREAMS: [(&str, &str, &str); 3] = [
    ("i", "s/i/ctrl", "s/i/!data"),
    ("v", "s/v/ctrl", "s/v/!data"),
    ("p", "s/p/ctrl", "s/p/!data"),
];
/// Current, voltage and power come at 1 MHz at most; the instrument's 2 MHz
/// is for its raw ADC channels only.
const RATES: [(u32, &str); 16] = [
    (1_000_000, "1MHz"),
    (500_000, "500kHz"),
    (200_000, "200kHz"),
    (100_000, "100kHz"),
    (50_000, "50kHz"),
    (20_000, "20kHz"),
    (10_000, "10kHz"),
    (5_000, "5kHz"),
    (2_000, "2kHz"),
    (1_000, "1kHz"),
    (500, "500Hz"),
    (200, "200Hz"),
    (100, "100Hz"),
    (50, "50Hz"),
    (20, "20Hz"),
    (10, "10Hz"),
];
pub const DEFAULT_RATE_HZ: u32 = 10_000;
/// `s/i/range/mode` values.
const MODE_OFF: u32 = 0;
const MODE_AUTO: u32 = 4;
const MODE_MANUAL: u32 = 5;
/// `s/i/range/select` values. "off" opens the current path: the target loses power.
const RANGES: [(u32, &str); 6] = [
    (129, "10A"),
    (130, "180mA"),
    (132, "18mA"),
    (136, "1.8mA"),
    (144, "180uA"),
    (160, "18uA"),
];
const VOLTAGE_RANGES: [&str; 3] = ["auto", "15V", "2V"];
const VREFS: [&str; 2] = ["external", "3.3V"];
const GPI_BITS: u8 = 4;
const GPO_COUNT: u8 = 2;

/// Connected JS220s, by the name the vendor driver gives them, e.g. `u/js220/005445`.
pub fn find() -> Vec<String> {
    jsdrv::driver()
        .map(|d| d.devices())
        .unwrap_or_default()
        .into_iter()
        .filter(|d| d.starts_with("u/js220/"))
        .collect()
}

/// The fields of the vendor's `jsdrv_stream_signal_s` that matter here.
#[derive(Debug, PartialEq)]
struct Samples<'a> {
    sample_id: u64,
    /// The rate `sample_id` counts at.
    sample_rate: u32,
    /// `sample_id` advances by this much per sample.
    decimate: u32,
    values: &'a [u8],
}
fn parse(m: &[u8]) -> Option<Samples<'_>> {
    const HEADER: usize = 56;
    let u32_at = |i: usize| Some(u32::from_le_bytes(m.get(i..i + 4)?.try_into().ok()?));
    let sample_id = u64::from_le_bytes(m.get(8..16)?.try_into().ok()?);
    let (float, bits) = (*m.get(18)?, *m.get(19)?);
    let (count, sample_rate, decimate) = (u32_at(20)?, u32_at(24)?, u32_at(28)?);
    let float32 = float == 4 && bits == 32;
    if !float32 || sample_rate == 0 || decimate == 0 {
        return None;
    }
    Some(Samples {
        sample_id,
        sample_rate,
        decimate,
        values: m.get(HEADER..HEADER + count as usize * 4)?,
    })
}
/// The driver marks samples it lost as NaN. They are left out, so each run of
/// real samples becomes a block of its own and the hole shows as a gap.
fn blocks(ch: u16, t: f64, dt: f64, values: &[u8], mut emit: impl FnMut(Block)) {
    let mut run: Vec<f32> = Vec::with_capacity(values.len() / 4);
    let mut start = 0;
    let mut flush = |run: &mut Vec<f32>, start: usize| {
        if !run.is_empty() {
            emit(Block {
                ch,
                t: t + start as f64 * dt,
                dt,
                data: Data::Analog(std::mem::take(run)),
            });
        }
    };
    for (k, b) in values.chunks_exact(4).enumerate() {
        let x = f32::from_le_bytes([b[0], b[1], b[2], b[3]]);
        if x.is_finite() {
            if run.is_empty() {
                start = k;
            }
            run.push(x);
        } else {
            flush(&mut run, start);
        }
    }
    flush(&mut run, start);
}

struct Stream {
    sink: Sender<Block>,
    /// The sample counter that is time 0.
    origin: Option<u64>,
}
struct Shared {
    device: String,
    subscribers: Mutex<Vec<Sender<Event>>>,
    stream: Mutex<Option<Stream>>,
    /// Last input mask sent as an event; `u32::MAX` before the first.
    inputs: AtomicU32,
}
impl Shared {
    fn broadcast(&self, event: Event) {
        self.subscribers
            .lock()
            .unwrap()
            .retain(|s| s.send(event.clone()).is_ok());
    }
}
fn shared<'a>(user: *const c_void) -> &'a Shared {
    // Every subscription is dropped before the `Shared` it points at.
    unsafe { &*(user as *const Shared) }
}
fn on_data(user: *const c_void, topic: &str, message: Message) {
    let s = shared(user);
    let Message::Bytes(bytes) = message else {
        return;
    };
    let Some(m) = parse(bytes) else { return };
    let Some(ch) = STREAMS.iter().position(|x| topic.ends_with(x.2)) else {
        return;
    };
    let mut stream = s.stream.lock().unwrap();
    let Some(stream) = stream.as_mut() else {
        return;
    };
    let origin = *stream.origin.get_or_insert(m.sample_id);
    let rate = f64::from(m.sample_rate);
    let t = m.sample_id.saturating_sub(origin) as f64 / rate;
    blocks(ch as u16, t, f64::from(m.decimate) / rate, m.values, |b| {
        let _ = stream.sink.send(b);
    });
}
/// Twice a second the instrument sends its own averages; they are the live values.
fn on_statistics(user: *const c_void, _: &str, message: Message) {
    let Message::Bytes(bytes) = message else {
        return;
    };
    if bytes.len() < size_of::<jsdrv::Statistics>() {
        return;
    }
    let st: jsdrv::Statistics = unsafe { std::ptr::read_unaligned(bytes.as_ptr().cast()) };
    for (ch, value) in [(CH_I, st.i_avg), (CH_V, st.v_avg), (CH_P, st.p_avg)] {
        shared(user).broadcast(Event::Reading {
            ch,
            value: value as f32,
        });
    }
}
fn on_inputs(user: *const c_void, _: &str, message: Message) {
    let Message::Number(mask) = message else {
        return;
    };
    let mask = mask as u32 & ((1 << GPI_BITS) - 1);
    let s = shared(user);
    if s.inputs.swap(mask, Ordering::Relaxed) != mask {
        s.broadcast(Event::Inputs(mask));
    }
}
fn on_removed(user: *const c_void, _: &str, message: Message) {
    let s = shared(user);
    if matches!(message, Message::Text(device) if device == s.device) {
        s.broadcast(Event::Disconnected);
    }
}

fn version(v: u32) -> String {
    format!("{}.{}.{}", v >> 24, (v >> 16) & 0xff, v & 0xffff)
}
fn choice(value: &Value, key: &str, options: &[&str]) -> Result<usize> {
    value
        .as_str()
        .and_then(|s| options.iter().position(|o| *o == s))
        .ok_or_else(|| Error::invalid(format!("{key} takes one of: {}", options.join(", "))))
}
fn range_options() -> Vec<&'static str> {
    ["off", "auto"]
        .into_iter()
        .chain(RANGES.iter().map(|r| r.1))
        .collect()
}

pub struct Joulescope {
    driver: &'static Driver,
    rate: AtomicU32,
    gpo: Mutex<[Option<bool>; GPO_COUNT as usize]>,
    /// What runs for as long as the device is open.
    watching: Vec<Subscription>,
    /// The streams of a running capture, with the topics that switch them off.
    streaming: Mutex<Vec<(&'static str, Subscription)>>,
    poll: Option<(Arc<AtomicBool>, thread::JoinHandle<()>)>,
    shared: Arc<Shared>,
}

impl Joulescope {
    /// `device` is a name from [`find`].
    pub fn open(device: &str) -> Result<Self> {
        let driver = jsdrv::driver()?;
        driver.open(device)?;
        let shared = Arc::new(Shared {
            device: device.into(),
            subscribers: Mutex::new(Vec::new()),
            stream: Mutex::new(None),
            inputs: AtomicU32::new(u32::MAX),
        });
        let mut js = Self {
            driver,
            rate: AtomicU32::new(DEFAULT_RATE_HZ),
            gpo: Mutex::new([None; GPO_COUNT as usize]),
            watching: Vec::new(),
            streaming: Mutex::new(Vec::new()),
            poll: None,
            shared,
        };
        // From here on dropping `js` closes the device again.
        let user = Arc::as_ptr(&js.shared).cast();
        js.publish("h/fs", DEFAULT_RATE_HZ)?;
        for (topic, callback) in [
            (js.topic("s/stats/value"), on_statistics as jsdrv::Callback),
            (js.topic("s/gpi/+/!value"), on_inputs),
            ("@/!remove".into(), on_removed),
        ] {
            js.watching.push(driver.subscribe(&topic, callback, user)?);
        }
        js.publish("s/stats/ctrl", 1)?;
        let stop = Arc::new(AtomicBool::new(false));
        let (flag, request) = (stop.clone(), js.topic("s/gpi/+/!req"));
        let poll = thread::spawn(move || {
            while !flag.load(Ordering::Relaxed) {
                let _ = driver.publish(&request, 1);
                thread::sleep(Duration::from_millis(200));
            }
        });
        js.poll = Some((stop, poll));
        Ok(js)
    }
    fn topic(&self, sub: &str) -> String {
        format!("{}/{sub}", self.shared.device)
    }
    fn publish(&self, sub: &str, v: u32) -> Result<()> {
        self.driver.publish(&self.topic(sub), v)
    }
    /// `None` when the instrument holds no value for the topic.
    fn query(&self, sub: &str) -> Result<Option<u32>> {
        self.driver.query(&self.topic(sub))
    }
    fn changed(&self) -> Result<()> {
        self.shared.broadcast(Event::Changed);
        Ok(())
    }
}
impl Drop for Joulescope {
    fn drop(&mut self) {
        let _ = Meter::stop(self);
        if let Some((stop, thread)) = self.poll.take() {
            stop.store(true, Ordering::Relaxed);
            let _ = thread.join();
        }
        let _ = self.publish("s/stats/ctrl", 0);
        self.watching.clear();
        self.driver.close(&self.shared.device);
    }
}

impl Device for Joulescope {
    fn info(&self) -> DeviceInfo {
        let read = |sub| {
            self.query(sub)
                .ok()
                .flatten()
                .map(version)
                .unwrap_or_default()
        };
        let mut details = serde_json::Map::new();
        details.insert("fpga".into(), read("s/fpga/version").into());
        DeviceInfo {
            kind: "joulescope".into(),
            model: "JS220".into(),
            serial: self
                .shared
                .device
                .rsplit('/')
                .next()
                .unwrap_or_default()
                .into(),
            firmware: read("c/fw/version"),
            hardware: read("c/hw/version"),
            details,
        }
    }
    fn channels(&self) -> Vec<ChannelDesc> {
        let rate = self.rate.load(Ordering::Relaxed);
        let labels = [("Current", "A"), ("Voltage", "V"), ("Power", "W")];
        STREAMS
            .iter()
            .zip(labels)
            .zip(0u16..)
            .map(|((s, (label, unit)), id)| {
                let mut c = ChannelDesc::new(id, s.0, label, unit, rate, Kind::Analog);
                c.group = "main".into();
                c
            })
            .collect()
    }
    fn settings(&self) -> Vec<SettingDesc> {
        let setting = |key: &str, label: &str, options: &[&str]| SettingDesc {
            key: key.into(),
            label: label.into(),
            kind: SettingKind::Choice {
                options: options.iter().map(|o| o.to_string()).collect(),
            },
            output: None,
            readonly: false,
        };
        let rates: Vec<_> = RATES.iter().map(|r| r.1).collect();
        vec![
            SettingDesc {
                kind: SettingKind::Bool,
                ..setting("power", "Target power", &[])
            },
            setting("rate", "Sample rate", &rates),
            setting("range", "Current range", &range_options()),
            setting("voltage_range", "Voltage range", &VOLTAGE_RANGES),
            setting("gpio_vref", "I/O reference", &VREFS),
        ]
    }
    fn get(&self, key: &str) -> Result<Value> {
        let name = match key {
            "rate" => {
                let hz = self.rate.load(Ordering::Relaxed);
                RATES.iter().find(|r| r.0 == hz).map(|r| r.1)
            }
            // The same switch as range "off", as the on/off it is in practice.
            "power" => {
                return Ok(self
                    .query("s/i/range/mode")?
                    .map_or(Value::Null, |mode| Value::Bool(mode != MODE_OFF)));
            }
            "range" => match self.query("s/i/range/mode")? {
                Some(MODE_OFF) => Some("off"),
                Some(MODE_AUTO) => Some("auto"),
                Some(MODE_MANUAL) => self
                    .query("s/i/range/select")?
                    .and_then(|select| RANGES.iter().find(|r| r.0 == select))
                    .map(|r| r.1),
                _ => None,
            },
            "voltage_range" => match self.query("s/v/range/mode")? {
                Some(0) => Some("auto"),
                Some(_) => self
                    .query("s/v/range/select")?
                    .and_then(|n| VOLTAGE_RANGES.get(1 + n as usize).copied()),
                None => None,
            },
            "gpio_vref" => self
                .query("c/gpio/vref")?
                .and_then(|n| VREFS.get(n as usize).copied()),
            _ => return Err(Error::invalid(format!("unknown setting {key}"))),
        };
        Ok(name.unwrap_or("unknown").into())
    }
    fn set(&self, key: &str, value: Value) -> Result<()> {
        match key {
            "rate" => {
                let names: Vec<_> = RATES.iter().map(|r| r.1).collect();
                let hz = RATES[choice(&value, key, &names)?].0;
                if self.shared.stream.lock().unwrap().is_some() {
                    return Err(Error::invalid("stop the capture before changing the rate"));
                }
                self.publish("h/fs", hz)?;
                self.rate.store(hz, Ordering::Relaxed);
            }
            "power" => match value.as_bool() {
                Some(false) => self.publish("s/i/range/mode", MODE_OFF)?,
                // Switching on keeps a range that is already closed.
                Some(true) if self.query("s/i/range/mode")? == Some(MODE_OFF) => {
                    self.publish("s/i/range/mode", MODE_AUTO)?
                }
                Some(true) => {}
                None => return Err(Error::invalid("power takes true or false")),
            },
            "range" => match choice(&value, key, &range_options())? {
                0 => self.publish("s/i/range/mode", MODE_OFF)?,
                1 => self.publish("s/i/range/mode", MODE_AUTO)?,
                n => {
                    self.publish("s/i/range/select", RANGES[n - 2].0)?;
                    self.publish("s/i/range/mode", MODE_MANUAL)?;
                }
            },
            "voltage_range" => match choice(&value, key, &VOLTAGE_RANGES)? {
                0 => self.publish("s/v/range/mode", 0)?,
                n => {
                    self.publish("s/v/range/select", n as u32 - 1)?;
                    self.publish("s/v/range/mode", 1)?;
                }
            },
            "gpio_vref" => self.publish("c/gpio/vref", choice(&value, key, &VREFS)? as u32)?,
            _ => return Err(Error::invalid(format!("unknown setting {key}"))),
        }
        self.changed()
    }
    fn events(&self) -> Receiver<Event> {
        let (tx, rx) = mpsc::channel();
        self.shared.subscribers.lock().unwrap().push(tx);
        rx
    }
    fn meter(&self) -> Option<&dyn Meter> {
        Some(self)
    }
    fn gpio(&self) -> Option<&dyn Gpio> {
        Some(self)
    }
}

impl Meter for Joulescope {
    fn start(&self, channels: &[u16], sink: Sender<Block>) -> Result<()> {
        self.stop()?;
        let mut wanted = Vec::new();
        for &ch in channels {
            let s = STREAMS
                .get(usize::from(ch))
                .ok_or_else(|| Error::invalid(format!("no channel {ch}")))?;
            wanted.push(s);
        }
        *self.shared.stream.lock().unwrap() = Some(Stream { sink, origin: None });
        let user = Arc::as_ptr(&self.shared).cast();
        let mut streaming = self.streaming.lock().unwrap();
        for (_, ctrl, data) in wanted {
            let s = self.driver.subscribe(&self.topic(data), on_data, user)?;
            streaming.push((ctrl, s));
        }
        // All streams are subscribed before the first starts, so they share time 0.
        for (ctrl, _) in streaming.iter() {
            self.publish(ctrl, 1)?;
        }
        Ok(())
    }
    fn stop(&self) -> Result<()> {
        let streaming = std::mem::take(&mut *self.streaming.lock().unwrap());
        let mut result = Ok(());
        for (ctrl, _) in &streaming {
            result = result.and(self.publish(ctrl, 0));
        }
        drop(streaming);
        *self.shared.stream.lock().unwrap() = None;
        result
    }
}

impl Gpio for Joulescope {
    fn outputs(&self) -> Vec<Option<bool>> {
        self.gpo.lock().unwrap().to_vec()
    }
    fn set_output(&self, n: u8, on: bool) -> Result<()> {
        if !(1..=GPO_COUNT).contains(&n) {
            return Err(Error::invalid(format!("no output {n}")));
        }
        let topic = if on { "s/gpo/+/!set" } else { "s/gpo/+/!clr" };
        self.publish(topic, 1 << (n - 1))?;
        self.gpo.lock().unwrap()[usize::from(n - 1)] = Some(on);
        self.changed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(sample_id: u64, decimate: u32, values: &[f32]) -> Vec<u8> {
        let mut m = vec![0u8; 56];
        m[0] = 1;
        m[8..16].copy_from_slice(&sample_id.to_le_bytes());
        m[16..20].copy_from_slice(&[1, 0, 4, 32]);
        m[20..24].copy_from_slice(&(values.len() as u32).to_le_bytes());
        m[24..28].copy_from_slice(&2_000_000u32.to_le_bytes());
        m[28..32].copy_from_slice(&decimate.to_le_bytes());
        m.extend(values.iter().flat_map(|v| v.to_le_bytes()));
        m
    }

    #[test]
    fn reads_the_stream_header() {
        let m = message(4000, 200, &[1.0, 2.0]);
        let s = parse(&m).unwrap();
        assert_eq!(
            (s.sample_id, s.sample_rate, s.decimate),
            (4000, 2_000_000, 200)
        );
        assert_eq!(s.values.len(), 8);
        // A message that is shorter than it says, or not float32, is dropped.
        assert_eq!(parse(&m[..60]), None);
        let mut bits = m.clone();
        bits[19] = 1;
        assert_eq!(parse(&bits), None);
    }

    #[test]
    fn lost_samples_split_a_block() {
        let m = message(0, 200, &[1.0, 2.0, f32::NAN, f32::NAN, 5.0]);
        let mut out = Vec::new();
        blocks(CH_I, 10.0, 1e-4, parse(&m).unwrap().values, |b| out.push(b));
        let block = |t, v: &[f32]| Block {
            ch: CH_I,
            t,
            dt: 1e-4,
            data: Data::Analog(v.to_vec()),
        };
        assert_eq!(out, [block(10.0, &[1.0, 2.0]), block(10.0 + 4e-4, &[5.0])]);
    }

    #[test]
    fn versions() {
        assert_eq!(version(16973827), "1.3.3");
    }
}
