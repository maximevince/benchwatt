//! Opens devices and runs captures: the one place where a driver meets a recording.
use benchwatt_device::{Block, ChannelDesc, Data, Device, Error, ErrorKind, Kind, Result};
use benchwatt_rec::{Recording, file};
use std::{
    sync::mpsc::{self, Receiver, RecvTimeoutError},
    time::{Duration, Instant},
};

pub mod serial;
use serial::SerialLink;

pub use benchwatt_device as device;
pub use benchwatt_rec as rec;

/// A supported device that is plugged in, not opened yet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    /// The driver's name, e.g. `arc`.
    pub kind: &'static str,
    pub port: String,
}
/// A driver: its name, what to call its devices before one is opened, how it
/// finds them by USB id and how it opens one.
type Driver = (
    &'static str,
    &'static str,
    fn() -> Vec<String>,
    fn(&str) -> Result<Box<dyn Device>>,
);
const DRIVERS: &[Driver] = &[
    ("arc", "Otii Arc", arc_ports, arc_open),
    ("ppk2", "Nordic PPK2", benchwatt_ppk2::find, ppk2_open),
    ("gpp", "GW Instek GPP", benchwatt_gpp::find, gpp_open),
    #[cfg(feature = "joulescope")]
    (
        "joulescope",
        "Joulescope JS220",
        benchwatt_joulescope::find,
        joulescope_open,
    ),
];
fn arc_ports() -> Vec<String> {
    otii_arc::device::find_port().into_iter().collect()
}
fn arc_open(port: &str) -> Result<Box<dyn Device>> {
    Ok(Box::new(otii_arc::ArcDevice::open(port)?))
}

fn ppk2_open(port: &str) -> Result<Box<dyn Device>> {
    Ok(Box::new(benchwatt_ppk2::Ppk2::open(port)?))
}

fn gpp_open(port: &str) -> Result<Box<dyn Device>> {
    Ok(Box::new(benchwatt_gpp::Gpp::open(port)?))
}

#[cfg(feature = "joulescope")]
fn joulescope_open(device: &str) -> Result<Box<dyn Device>> {
    Ok(Box::new(benchwatt_joulescope::Joulescope::open(device)?))
}

pub fn find() -> Vec<Found> {
    DRIVERS
        .iter()
        .flat_map(|&(kind, _, ports, _)| ports().into_iter().map(move |port| Found { kind, port }))
        .collect()
}
impl Found {
    /// What to call the device while it is not open, e.g. `Otii Arc`.
    pub fn name(&self) -> &'static str {
        DRIVERS
            .iter()
            .find(|d| d.0 == self.kind)
            .map_or(self.kind, |d| d.1)
    }
}
pub fn open(found: &Found) -> Result<Box<dyn Device>> {
    let (_, _, _, open) = DRIVERS
        .iter()
        .find(|d| d.0 == found.kind)
        .ok_or_else(|| Error::invalid(format!("unknown device kind {}", found.kind)))?;
    open(&found.port)
}
/// Pick one device: `kind`, `kind@port` for a device that cannot be found by
/// USB id, or nothing when only one is plugged in.
pub fn select(device: Option<&str>) -> Result<Found> {
    let found = find();
    let Some(device) = device else {
        return match found.as_slice() {
            [one] => Ok(one.clone()),
            [] => Err(Error::new(ErrorKind::Io, "no device found")),
            many => Err(Error::invalid(format!(
                "several devices found, name one: {}",
                many.iter().map(|f| f.kind).collect::<Vec<_>>().join(", ")
            ))),
        };
    };
    let (kind, port) = match device.split_once('@') {
        Some((kind, port)) => (kind, Some(port)),
        None => (device, None),
    };
    let kind = DRIVERS
        .iter()
        .find(|d| d.0 == kind)
        .ok_or_else(|| Error::invalid(format!("unknown device kind {kind}")))?
        .0;
    match port {
        Some(port) => Ok(Found {
            kind,
            port: port.into(),
        }),
        None => found
            .into_iter()
            .find(|f| f.kind == kind)
            .ok_or_else(|| Error::new(ErrorKind::Io, format!("no {kind} found"))),
    }
}
pub fn channel(dev: &dyn Device, name: &str) -> Result<ChannelDesc> {
    dev.channels()
        .into_iter()
        .find(|c| c.name == name)
        .ok_or_else(|| Error::invalid(format!("unknown channel {name}")))
}
/// What a saved recording says about the device it came from.
pub fn meta(dev: &dyn Device) -> file::Meta {
    let info = dev.info();
    let text = |s: String| (!s.is_empty()).then_some(s);
    file::Meta {
        device: file::DeviceInfo {
            kind: text(info.kind),
            model: text(info.model),
            serial: text(info.serial),
            firmware: text(info.firmware),
            hardware: text(info.hardware),
        },
        ..file::Meta::default()
    }
}
/// Write to the device's UART and log the bytes in the recording, if there is one.
pub fn uart_write(dev: &dyn Device, rec: Option<&mut Recording>, bytes: &[u8]) -> Result<()> {
    let uart = dev
        .uart()
        .ok_or_else(|| Error::invalid("device has no UART"))?;
    uart.write(bytes)?;
    if let Some(rec) = rec
        && let Some(tx) = dev.channels().into_iter().find(|c| c.id == uart.tx())
    {
        rec.push_tx(&tx, bytes);
    }
    Ok(())
}

/// Write to a linked serial port and log the bytes in the recording, if there is one.
pub fn serial_write(link: &SerialLink, rec: Option<&mut Recording>, bytes: &[u8]) -> Result<()> {
    link.write(bytes)?;
    if let Some(rec) = rec {
        rec.push_tx(&serial::channels()[1], bytes);
    }
    Ok(())
}

/// Where the recording's time zero lies on the PC's clock, estimated from when
/// the device's samples arrive. A sample taken at recording time `t` cannot
/// arrive before `zero + t`, so the earliest arrival seen is the best estimate.
/// Only the last few seconds count, because the two clocks drift apart.
#[derive(Default)]
struct Origin {
    /// Time of the newest device sample.
    newest: f64,
    window: Option<(Instant, Instant)>,
    previous: Option<Instant>,
}
impl Origin {
    const WINDOW: Duration = Duration::from_secs(2);
    fn sample(&mut self, b: &Block) {
        let n = match &b.data {
            Data::Analog(v) => v.len(),
            Data::Digital(v) => v.len(),
            Data::Bytes(_) => 1,
        };
        self.newest = self.newest.max(b.t + n.saturating_sub(1) as f64 * b.dt);
    }
    /// Samples up to `newest` have arrived by `now`.
    fn arrived(&mut self, now: Instant) {
        let Some(zero) = now.checked_sub(Duration::from_secs_f64(self.newest.max(0.0))) else {
            return;
        };
        match &mut self.window {
            Some((start, best)) if now.duration_since(*start) < Self::WINDOW => {
                *best = (*best).min(zero)
            }
            window => {
                self.previous = window.map(|(_, best)| best);
                *window = Some((now, zero));
            }
        }
    }
    fn zero(&self) -> Option<Instant> {
        let current = self.window.map(|(_, best)| best)?;
        Some(self.previous.map_or(current, |p| p.min(current)))
    }
}
/// A serial port feeding a capture.
struct Linked {
    chunks: Receiver<serial::Chunk>,
    /// Time of the last byte chunk stored, so their order survives a moving estimate.
    last: f64,
}

/// A running measurement: blocks from the device's meter on their way into a recording.
pub struct Capture {
    blocks: Receiver<Block>,
    uart: bool,
    origin: Origin,
    serial: Option<Linked>,
}
impl Capture {
    /// Start the meter on the named channels, plus the digital inputs and, with a
    /// baud rate, the UART, which is then opened here and closed by `stop`.
    pub fn start<S: AsRef<str>>(
        dev: &dyn Device,
        names: &[S],
        digital: bool,
        uart: Option<u32>,
    ) -> Result<(Self, Recording)> {
        let all = dev.channels();
        let mut chosen = Vec::new();
        for name in names {
            chosen.push(channel(dev, name.as_ref())?);
        }
        if digital {
            chosen.extend(all.iter().filter(|c| c.kind == Kind::Digital).cloned());
        }
        let port = match uart {
            Some(_) => Some(
                dev.uart()
                    .ok_or_else(|| Error::invalid("device has no UART"))?,
            ),
            None => None,
        };
        if let Some(port) = port {
            chosen.extend(all.iter().filter(|c| c.id == port.rx()).cloned());
        }
        chosen.sort_by_key(|c| c.id);
        chosen.dedup_by_key(|c| c.id);
        if chosen.is_empty() {
            return Err(Error::invalid("no channels selected"));
        }
        let meter = dev
            .meter()
            .ok_or_else(|| Error::invalid("device cannot measure"))?;
        if let (Some(port), Some(baud)) = (port, uart) {
            port.open(baud)?;
        }
        let (sink, blocks) = mpsc::channel();
        let ids: Vec<u16> = chosen.iter().map(|c| c.id).collect();
        if let Err(e) = meter.start(&ids, sink) {
            if let Some(port) = port {
                let _ = port.close();
            }
            return Err(e);
        }
        Ok((
            Self {
                blocks,
                uart: uart.is_some(),
                origin: Origin::default(),
                serial: None,
            },
            Recording::new(&chosen),
        ))
    }
    /// Record what arrives on a serial port from now on, as `serial.rx`. The
    /// PC stamps the bytes when it reads them, so they sit within a few
    /// milliseconds of the device's samples, not exactly on them.
    pub fn attach(&mut self, link: &SerialLink, rec: &mut Recording) {
        rec.add_channel(serial::channels()[0].clone());
        self.serial = Some(Linked {
            chunks: link.attach(),
            last: 0.0,
        });
    }
    fn push(&mut self, rec: &mut Recording, b: &Block) {
        self.origin.sample(b);
        rec.push(b);
    }
    /// Store the serial bytes that have arrived, once the device has shown where time zero is.
    fn pump_serial(&mut self, rec: &mut Recording) {
        let (Some(linked), Some(zero)) = (self.serial.as_mut(), self.origin.zero()) else {
            return;
        };
        while let Ok((at, bytes)) = linked.chunks.try_recv() {
            linked.last = linked
                .last
                .max(at.saturating_duration_since(zero).as_secs_f64());
            rec.push(&Block {
                ch: serial::RX_ID,
                t: linked.last,
                dt: 0.0,
                data: Data::Bytes(bytes),
            });
        }
    }
    /// Move the blocks that have arrived into the recording, waiting up to `wait`
    /// for the first. Returns false once the device is gone.
    pub fn pump(&mut self, rec: &mut Recording, wait: Duration) -> bool {
        match self.blocks.recv_timeout(wait) {
            Ok(b) => {
                self.push(rec, &b);
                while let Ok(b) = self.blocks.try_recv() {
                    self.push(rec, &b)
                }
                self.origin.arrived(Instant::now());
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return false,
        }
        self.pump_serial(rec);
        true
    }
    /// Stop the meter, keep what it still delivered and end the recording.
    pub fn stop(mut self, dev: &dyn Device, rec: &mut Recording) -> Result<()> {
        let stopped = dev.meter().map_or(Ok(()), |m| m.stop());
        while let Ok(b) = self.blocks.try_recv() {
            self.push(rec, &b)
        }
        // A capture that was never pumped gets its time zero here.
        self.origin.arrived(Instant::now());
        self.pump_serial(rec);
        rec.stop();
        if self.uart
            && let Some(port) = dev.uart()
        {
            let _ = port.close();
        }
        stopped
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use benchwatt_device::{DeviceInfo, Event, Meter, Value};
    use serialport::{SerialPort, TTYPort};
    use std::{
        io::Write,
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
            mpsc::Sender,
        },
        thread,
    };

    /// A meter that streams one 1 kHz channel in 10 ms blocks.
    struct Fake(Arc<AtomicBool>);
    impl Device for Fake {
        fn info(&self) -> DeviceInfo {
            DeviceInfo::default()
        }
        fn channels(&self) -> Vec<ChannelDesc> {
            vec![ChannelDesc::new(0, "i", "Current", "A", 1000, Kind::Analog)]
        }
        fn get(&self, _: &str) -> Result<Value> {
            Err(Error::invalid("no settings"))
        }
        fn set(&self, _: &str, _: Value) -> Result<()> {
            Err(Error::invalid("no settings"))
        }
        fn events(&self) -> Receiver<Event> {
            mpsc::channel().1
        }
        fn meter(&self) -> Option<&dyn Meter> {
            Some(self)
        }
    }
    impl Meter for Fake {
        fn start(&self, _: &[u16], sink: Sender<Block>) -> Result<()> {
            let running = self.0.clone();
            running.store(true, Ordering::Release);
            thread::spawn(move || {
                let start = Instant::now();
                for i in 0.. {
                    let t = i as f64 * 0.01;
                    // The block is complete 10 ms after its first sample.
                    thread::sleep(
                        (start + Duration::from_secs_f64(t + 0.01))
                            .saturating_duration_since(Instant::now()),
                    );
                    let block = Block {
                        ch: 0,
                        t,
                        dt: 0.001,
                        data: Data::Analog(vec![0.0; 10]),
                    };
                    if !running.load(Ordering::Acquire) || sink.send(block).is_err() {
                        break;
                    }
                }
            });
            Ok(())
        }
        fn stop(&self) -> Result<()> {
            self.0.store(false, Ordering::Release);
            Ok(())
        }
    }

    #[test]
    fn serial_bytes_land_on_the_recording_time_axis() {
        let (mut host, target) = TTYPort::pair().unwrap();
        let path = target.name().unwrap();
        drop(target);
        let link = SerialLink::open(&path, &serial::Settings::default()).unwrap();
        let dev = Fake(Arc::default());
        let (mut capture, mut rec) = Capture::start(&dev, &["i"], false, None).unwrap();
        capture.attach(&link, &mut rec);
        let started = Instant::now();
        let mut sent = Vec::new();
        for line in ["one\n", "two\n"] {
            while started.elapsed() < Duration::from_millis(150 * (sent.len() as u64 + 1)) {
                capture.pump(&mut rec, Duration::from_millis(5));
            }
            sent.push(started.elapsed().as_secs_f64());
            host.write_all(line.as_bytes()).unwrap();
        }
        while started.elapsed() < Duration::from_millis(400) {
            capture.pump(&mut rec, Duration::from_millis(5));
        }
        serial_write(&link, Some(&mut rec), b"ok\n").unwrap();
        capture.stop(&dev, &mut rec).unwrap();

        let rx = rec.bytes(serial::RX_ID, 0.0, 10.0);
        let text: Vec<u8> = rx.iter().flat_map(|c| c.bytes.clone()).collect();
        assert_eq!(text, b"one\ntwo\n");
        assert_eq!(rx.len(), 2);
        for (chunk, at) in rx.iter().zip(sent) {
            assert!((chunk.t - at).abs() < 0.03, "{} vs {at}", chunk.t);
        }
        assert_eq!(rec.bytes(serial::TX_ID, 0.0, 10.0)[0].bytes, b"ok\n");
        let mut echoed = [0u8; 3];
        std::io::Read::read_exact(&mut host, &mut echoed).unwrap();
        assert_eq!(&echoed, b"ok\n");
        let names: Vec<String> = rec.specs().into_iter().map(|c| c.name).collect();
        assert_eq!(names, ["i", "serial.rx", "serial.tx"]);
    }
    #[test]
    fn the_origin_follows_the_earliest_arrival() {
        let mut origin = Origin::default();
        let base = Instant::now();
        let at = |ms: u64| base + Duration::from_millis(ms);
        let block = |t: f64| Block {
            ch: 0,
            t,
            dt: 0.001,
            data: Data::Analog(vec![0.0; 1]),
        };
        assert!(origin.zero().is_none());
        // Time zero is `base`; the first block arrives 30 ms late, the next 5 ms late.
        origin.sample(&block(1.0));
        origin.arrived(at(1030));
        assert_eq!(origin.zero(), Some(at(30)));
        origin.sample(&block(1.5));
        origin.arrived(at(1505));
        assert_eq!(origin.zero(), Some(at(5)));
        // A later, worse estimate does not move it while the good one is recent.
        origin.sample(&block(4.0));
        origin.arrived(at(4020));
        assert_eq!(origin.zero(), Some(at(5)));
        // Once the good one is two windows old, the clock is followed again.
        origin.sample(&block(7.0));
        origin.arrived(at(7020));
        assert_eq!(origin.zero(), Some(at(20)));
    }
}
