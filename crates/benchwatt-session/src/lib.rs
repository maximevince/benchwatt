//! Opens devices and runs captures: the one place where a driver meets a recording.
use benchwatt_device::{Block, ChannelDesc, Device, Error, ErrorKind, Kind, Result};
use benchwatt_rec::{Recording, file};
use std::{
    sync::mpsc::{self, Receiver, RecvTimeoutError},
    time::Duration,
};

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

/// A running measurement: blocks from the device's meter on their way into a recording.
pub struct Capture {
    blocks: Receiver<Block>,
    uart: bool,
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
            },
            Recording::new(&chosen),
        ))
    }
    /// Move the blocks that have arrived into the recording, waiting up to `wait`
    /// for the first. Returns false once the device is gone.
    pub fn pump(&self, rec: &mut Recording, wait: Duration) -> bool {
        match self.blocks.recv_timeout(wait) {
            Ok(b) => rec.push(&b),
            Err(RecvTimeoutError::Timeout) => return true,
            Err(RecvTimeoutError::Disconnected) => return false,
        }
        while let Ok(b) = self.blocks.try_recv() {
            rec.push(&b)
        }
        true
    }
    /// Stop the meter, keep what it still delivered and end the recording.
    pub fn stop(self, dev: &dyn Device, rec: &mut Recording) -> Result<()> {
        let stopped = dev.meter().map_or(Ok(()), |m| m.stop());
        while let Ok(b) = self.blocks.try_recv() {
            rec.push(&b)
        }
        rec.stop();
        if self.uart
            && let Some(port) = dev.uart()
        {
            let _ = port.close();
        }
        stopped
    }
}
