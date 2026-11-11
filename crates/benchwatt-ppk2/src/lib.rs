//! Nordic Power Profiler Kit II behind the device-neutral traits of `benchwatt-device`.
mod proto;
pub use proto::{Calibration, Sample, SpikeFilter, convert, lost_samples, parse_calibration};

use benchwatt_device::{
    Block, ChannelDesc, Data, Device, DeviceInfo, Error, ErrorKind, Event, Kind, Meter, OutputDesc,
    OutputState, Range, Result, SettingDesc, SettingKind, Supply, Value,
};
use serialport::SerialPortType;
use std::{
    io::{self, Read, Write},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread,
    time::{Duration, Instant},
};

pub const VID: u16 = 0x1915;
pub const PID: u16 = 0xc00a;
pub const CH_I: u16 = 0;
pub const CH_V: u16 = 1;
pub const CH_D: u16 = 2;
pub const RATE_HZ: u32 = 100_000;
/// Samples per block: 10 ms.
pub const BLOCK: usize = 1000;

const START: u8 = 0x06;
const STOP: u8 = 0x07;
const POWER: u8 = 0x0C;
const VOLTAGE: u8 = 0x0D;
const MODE: u8 = 0x11;
const CALIBRATION: u8 = 0x19;
const VOLTS: Range = Range { min: 0.8, max: 5.0 };
const MODES: [&str; 2] = ["source", "ampere"];
/// The unit's power-on voltage when the calibration reply does not say.
const DEFAULT_MV: u16 = 3700;

/// Serial ports of connected PPK2s. Each unit has two; only USB interface 1
/// speaks the protocol.
pub fn find() -> Vec<String> {
    serialport::available_ports()
        .unwrap_or_default()
        .into_iter()
        .filter(|p| match &p.port_type {
            SerialPortType::UsbPort(u) => {
                u.vid == VID && u.pid == PID && u.interface.is_none_or(|i| i == 1)
            }
            _ => false,
        })
        .map(|p| p.port_name)
        .collect()
}

type Reader = Box<dyn Read + Send>;

fn io_error(e: io::Error) -> Error {
    Error::new(ErrorKind::Io, e.to_string())
}
fn idle(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
    )
}
/// Read and drop whatever arrives until the line goes quiet for one read timeout.
fn discard(reader: &mut Reader) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut buf = [0u8; 4096];
    while Instant::now() < deadline {
        match reader.read(&mut buf) {
            Ok(0) => return Ok(()),
            Ok(_) => {}
            Err(e) if idle(&e) => return Ok(()),
            Err(e) => return Err(io_error(e)),
        }
    }
    Ok(())
}

struct Shared {
    writer: Mutex<Box<dyn Write + Send>>,
    subscribers: Mutex<Vec<Sender<Event>>>,
    vdd_mv: AtomicU32,
    spike_filter: AtomicBool,
}
impl Shared {
    fn write(&self, bytes: &[u8]) -> Result<()> {
        let mut w = self.writer.lock().unwrap();
        w.write_all(bytes).and_then(|_| w.flush()).map_err(io_error)
    }
    fn broadcast(&self, event: Event) {
        self.subscribers
            .lock()
            .unwrap()
            .retain(|s| s.send(event.clone()).is_ok());
    }
}

struct Output {
    enabled: bool,
    ampere: bool,
}
struct Streaming {
    stop: Arc<AtomicBool>,
    thread: thread::JoinHandle<Reader>,
}

pub struct Ppk2 {
    shared: Arc<Shared>,
    cal: Arc<Calibration>,
    serial: String,
    output: Mutex<Output>,
    reader: Mutex<Option<Reader>>,
    stream: Mutex<Option<Streaming>>,
}

impl Ppk2 {
    pub fn open(port: &str) -> Result<Self> {
        let p = serialport::new(port, 115200)
            .timeout(Duration::from_millis(50))
            .open()
            .map_err(|e| Error::new(ErrorKind::Io, e.to_string()))?;
        let reader = p
            .try_clone()
            .map_err(|e| Error::new(ErrorKind::Io, e.to_string()))?;
        let mut ppk = Self::from_io(reader, p)?;
        ppk.serial = serialport::available_ports()
            .unwrap_or_default()
            .into_iter()
            .find_map(|p| match p.port_type {
                SerialPortType::UsbPort(u) if p.port_name == port => u.serial_number,
                _ => None,
            })
            .unwrap_or_default();
        Ok(ppk)
    }
    /// Talk to a PPK2 over any byte stream. Reads should time out (as
    /// `TimedOut` or `WouldBlock`) rather than block forever. Reads the
    /// calibration before returning.
    pub fn from_io<R: Read + Send + 'static, W: Write + Send + 'static>(
        reader: R,
        writer: W,
    ) -> Result<Self> {
        let mut reader: Reader = Box::new(reader);
        let shared = Arc::new(Shared {
            writer: Mutex::new(Box::new(writer)),
            subscribers: Mutex::new(Vec::new()),
            vdd_mv: AtomicU32::new(0),
            spike_filter: AtomicBool::new(true),
        });
        let cal = calibrate(&shared, &mut reader)?;
        shared
            .vdd_mv
            .store(cal.vdd_mv.unwrap_or(DEFAULT_MV).into(), Ordering::Relaxed);
        Ok(Self {
            output: Mutex::new(Output {
                enabled: false,
                ampere: cal.mode == Some(1),
            }),
            shared,
            cal: Arc::new(cal),
            serial: String::new(),
            reader: Mutex::new(Some(reader)),
            stream: Mutex::new(None),
        })
    }
    pub fn calibration(&self) -> &Calibration {
        &self.cal
    }
    fn send_voltage(&self) -> Result<()> {
        let [hi, lo] = (self.shared.vdd_mv.load(Ordering::Relaxed) as u16).to_be_bytes();
        self.shared.write(&[VOLTAGE, hi, lo])
    }
}

fn calibrate(shared: &Shared, reader: &mut Reader) -> Result<Calibration> {
    let mut last = Error::new(ErrorKind::Timeout, "no calibration reply");
    for _ in 0..3 {
        shared.write(&[STOP])?;
        thread::sleep(Duration::from_millis(50));
        discard(reader)?;
        shared.write(&[CALIBRATION])?;
        let deadline = Instant::now() + Duration::from_secs(1);
        let mut reply = Vec::new();
        let mut buf = [0u8; 2048];
        while Instant::now() < deadline {
            match reader.read(&mut buf) {
                Ok(n) => reply.extend_from_slice(&buf[..n]),
                Err(e) if idle(&e) => {}
                Err(e) => return Err(io_error(e)),
            }
            if String::from_utf8_lossy(&reply)
                .lines()
                .any(|l| l.trim() == "END")
            {
                break;
            }
        }
        match parse_calibration(&String::from_utf8_lossy(&reply)) {
            Ok(cal) => return Ok(cal),
            Err(e) => last = e,
        }
    }
    Err(Error::new(
        ErrorKind::Timeout,
        format!("PPK2 calibration failed after 3 attempts: {last}"),
    ))
}

/// Turns sample words into blocks on the device's own sample clock.
struct Assembler {
    cal: Arc<Calibration>,
    shared: Arc<Shared>,
    sink: Sender<Block>,
    want: [bool; 3],
    filter: SpikeFilter,
    expected: Option<u8>,
    /// Index of the next sample since start, lost ones included.
    k: u64,
    first: u64,
    i: Vec<f32>,
    d: Vec<u32>,
    sum: f64,
}
impl Assembler {
    fn push(&mut self, word: u32, vdd_mv: f64, filter: bool) {
        let s = convert(word, &self.cal, vdd_mv);
        if let Some(expected) = self.expected {
            let lost = lost_samples(expected, s.counter);
            if lost > 0 {
                self.flush(vdd_mv);
                self.k += u64::from(lost);
            }
        }
        self.expected = Some((s.counter + 1) & 0x3f);
        let amps = if filter {
            self.filter.apply(s.amps, s.range)
        } else {
            s.amps
        };
        if self.i.is_empty() {
            self.first = self.k;
        }
        self.i.push(amps as f32);
        self.d.push(s.bits.into());
        self.sum += amps;
        self.k += 1;
        if self.i.len() == BLOCK {
            self.flush(vdd_mv);
        }
    }
    fn flush(&mut self, vdd_mv: f64) {
        let n = self.i.len();
        if n == 0 {
            return;
        }
        let rate = f64::from(RATE_HZ);
        let t = self.first as f64 / rate;
        let i = std::mem::replace(&mut self.i, Vec::with_capacity(BLOCK));
        let d = std::mem::replace(&mut self.d, Vec::with_capacity(BLOCK));
        let mean = std::mem::take(&mut self.sum) / n as f64;
        let block = |ch, dt, data| Block { ch, t, dt, data };
        if self.want[CH_I as usize] {
            let _ = self.sink.send(block(CH_I, 1.0 / rate, Data::Analog(i)));
        }
        if self.want[CH_V as usize] {
            let v = vec![(vdd_mv / 1000.0) as f32];
            let _ = self
                .sink
                .send(block(CH_V, n as f64 / rate, Data::Analog(v)));
        }
        if self.want[CH_D as usize] {
            let _ = self.sink.send(block(CH_D, 1.0 / rate, Data::Digital(d)));
        }
        self.shared.broadcast(Event::Reading {
            ch: CH_I,
            value: mean as f32,
        });
    }
}

fn read_stream(mut reader: Reader, mut a: Assembler, stop: Arc<AtomicBool>) -> Reader {
    let mut buf = vec![0u8; 16384];
    let mut carry = 0;
    let mut vdd_mv = f64::from(a.shared.vdd_mv.load(Ordering::Relaxed));
    while !stop.load(Ordering::Relaxed) {
        let n = match reader.read(&mut buf[carry..]) {
            Ok(n) => n,
            Err(e) if idle(&e) => 0,
            Err(_) => {
                a.flush(vdd_mv);
                a.shared.broadcast(Event::Disconnected);
                return reader;
            }
        };
        if n == 0 {
            continue;
        }
        vdd_mv = f64::from(a.shared.vdd_mv.load(Ordering::Relaxed));
        let filter = a.shared.spike_filter.load(Ordering::Relaxed);
        let total = carry + n;
        let whole = total - total % 4;
        for w in buf[..whole].chunks_exact(4) {
            a.push(u32::from_le_bytes([w[0], w[1], w[2], w[3]]), vdd_mv, filter);
        }
        buf.copy_within(whole..total, 0);
        carry = total - whole;
    }
    a.flush(vdd_mv);
    reader
}

fn check_out(out: u8) -> Result<()> {
    if out == 1 {
        Ok(())
    } else {
        Err(Error::invalid(format!("no output {out}")))
    }
}

impl Device for Ppk2 {
    fn info(&self) -> DeviceInfo {
        let c = &self.cal;
        let mut details = serde_json::Map::new();
        details.insert("Calibrated".into(), c.calibrated.into());
        details.insert("VDD".into(), c.vdd_mv.into());
        details.insert("mode".into(), c.mode.into());
        details.insert("IA".into(), c.ia.into());
        DeviceInfo {
            kind: "ppk2".into(),
            model: "PPK2".into(),
            serial: self.serial.clone(),
            firmware: String::new(),
            hardware: c.hw.clone().unwrap_or_default(),
            details,
        }
    }
    fn channels(&self) -> Vec<ChannelDesc> {
        vec![
            ChannelDesc {
                group: "out".into(),
                ..ChannelDesc::new(CH_I, "i", "Current", "A", RATE_HZ, Kind::Analog)
            },
            ChannelDesc {
                group: "out".into(),
                set: true,
                ..ChannelDesc::new(CH_V, "v", "Voltage", "V", 100, Kind::Analog)
            },
            ChannelDesc {
                bits: 8,
                ..ChannelDesc::new(CH_D, "d", "Digital inputs", "", RATE_HZ, Kind::Digital)
            },
        ]
    }
    fn settings(&self) -> Vec<SettingDesc> {
        vec![
            SettingDesc {
                key: "mode".into(),
                label: "Mode".into(),
                kind: SettingKind::Choice {
                    options: MODES.iter().map(|m| (*m).into()).collect(),
                },
                output: Some(1),
                readonly: false,
            },
            SettingDesc {
                key: "spike_filter".into(),
                label: "Spike filter".into(),
                kind: SettingKind::Bool,
                output: None,
                readonly: false,
            },
        ]
    }
    fn get(&self, key: &str) -> Result<Value> {
        match key {
            "mode" => Ok(MODES[usize::from(self.output.lock().unwrap().ampere)].into()),
            "spike_filter" => Ok(self.shared.spike_filter.load(Ordering::Relaxed).into()),
            _ => Err(Error::invalid(format!("unknown setting {key}"))),
        }
    }
    fn set(&self, key: &str, value: Value) -> Result<()> {
        match (key, &value) {
            ("mode", Value::String(m)) => {
                let ampere = match m.as_str() {
                    "source" => false,
                    "ampere" => true,
                    _ => return Err(Error::invalid("mode must be source or ampere")),
                };
                let mut out = self.output.lock().unwrap();
                self.shared.write(&[MODE, if ampere { 1 } else { 2 }])?;
                out.ampere = ampere;
                // In ampere-meter mode the DUT stays unpowered until a voltage is sent.
                if ampere {
                    self.send_voltage()?;
                }
            }
            ("spike_filter", Value::Bool(on)) => {
                self.shared.spike_filter.store(*on, Ordering::Relaxed)
            }
            ("mode" | "spike_filter", _) => {
                return Err(Error::invalid(format!("wrong value type for {key}")));
            }
            _ => return Err(Error::invalid(format!("unknown setting {key}"))),
        }
        self.shared.broadcast(Event::Changed);
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
}

impl Supply for Ppk2 {
    fn outputs(&self) -> Vec<OutputDesc> {
        vec![OutputDesc {
            n: 1,
            label: "Output".into(),
            group: "out".into(),
            voltage: VOLTS,
            voltage_steps: Vec::new(),
            current_limit: None,
        }]
    }
    fn set_voltage(&self, out: u8, volts: f64) -> Result<()> {
        check_out(out)?;
        if !VOLTS.contains(volts) {
            return Err(Error::invalid(format!(
                "voltage must be {} to {} V",
                VOLTS.min, VOLTS.max
            )));
        }
        let mv = (volts * 1000.0).round() as u16;
        let [hi, lo] = mv.to_be_bytes();
        self.shared.write(&[VOLTAGE, hi, lo])?;
        self.shared.vdd_mv.store(mv.into(), Ordering::Relaxed);
        self.shared.broadcast(Event::Changed);
        Ok(())
    }
    fn set_current_limit(&self, out: u8, _amps: f64) -> Result<()> {
        check_out(out)?;
        Err(Error::invalid("the PPK2 has no current limit"))
    }
    fn set_enabled(&self, out: u8, on: bool) -> Result<()> {
        check_out(out)?;
        let mut o = self.output.lock().unwrap();
        self.shared.write(&[POWER, on.into()])?;
        o.enabled = on;
        drop(o);
        self.shared.broadcast(Event::Changed);
        Ok(())
    }
    fn state(&self, out: u8) -> Result<OutputState> {
        check_out(out)?;
        Ok(OutputState {
            enabled: self.output.lock().unwrap().enabled,
            voltage: f64::from(self.shared.vdd_mv.load(Ordering::Relaxed)) / 1000.0,
            current_limit: 0.0,
        })
    }
}

impl Meter for Ppk2 {
    fn start(&self, channels: &[u16], sink: Sender<Block>) -> Result<()> {
        let mut want = [false; 3];
        for &ch in channels {
            *want
                .get_mut(usize::from(ch))
                .ok_or_else(|| Error::invalid(format!("no channel {ch}")))? = true;
        }
        let mut stream = self.stream.lock().unwrap();
        if stream.is_some() {
            return Err(Error::invalid("already streaming"));
        }
        let mut slot = self.reader.lock().unwrap();
        let reader = slot
            .take()
            .ok_or_else(|| Error::new(ErrorKind::Closed, "port closed"))?;
        if let Err(e) = self.shared.write(&[START]) {
            *slot = Some(reader);
            return Err(e);
        }
        let assembler = Assembler {
            cal: self.cal.clone(),
            shared: self.shared.clone(),
            sink,
            want,
            filter: SpikeFilter::default(),
            expected: None,
            k: 0,
            first: 0,
            i: Vec::with_capacity(BLOCK),
            d: Vec::with_capacity(BLOCK),
            sum: 0.0,
        };
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        *stream = Some(Streaming {
            stop,
            thread: thread::spawn(move || read_stream(reader, assembler, flag)),
        });
        Ok(())
    }
    fn stop(&self) -> Result<()> {
        let Some(s) = self.stream.lock().unwrap().take() else {
            return Ok(());
        };
        let sent = self.shared.write(&[STOP]);
        s.stop.store(true, Ordering::Relaxed);
        let mut reader = s
            .thread
            .join()
            .map_err(|_| Error::new(ErrorKind::Io, "PPK2 reader thread panicked"))?;
        let discarded = discard(&mut reader);
        *self.reader.lock().unwrap() = Some(reader);
        sent.and(discarded)
    }
}

impl Drop for Ppk2 {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}
