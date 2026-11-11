//! Opens devices and runs captures: the one place where a driver meets a recording.
use benchwatt_device::{Block, ChannelDesc, Device, Error, Kind, Result};
use benchwatt_rec::{Recording, file};
use std::{
    sync::mpsc::{self, Receiver, RecvTimeoutError},
    time::Duration,
};

pub use benchwatt_device as device;
pub use benchwatt_rec as rec;

/// The port of the first supported device that is plugged in.
pub fn find() -> Option<String> {
    otii_arc::device::find_port()
}
pub fn open(port: &str) -> Result<Box<dyn Device>> {
    Ok(Box::new(otii_arc::ArcDevice::open(port)?))
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
            name: text(info.model),
            fw: text(info.firmware),
            hw: text(info.hardware),
            uid: text(info.serial),
            proto: info.details["protocol"].as_u64().map(|p| p as u32),
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
