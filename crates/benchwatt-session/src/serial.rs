//! A serial port of the PC recorded next to an instrument: what the target
//! prints on it lands in the recording as `serial.rx`, what is written to it
//! as `serial.tx`.
use benchwatt_device::{ChannelDesc, Error, ErrorKind, Kind, Result};
use serde::{Deserialize, Serialize};
use serialport::{SerialPort, SerialPortType};
use std::{
    io::{self, Read},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

/// Channel ids at the top of the range, where no driver puts its own.
pub const RX_ID: u16 = 0xfff0;
pub const TX_ID: u16 = 0xfff1;
/// Bytes as they arrived, with the time the read returned.
pub(crate) type Chunk = (Instant, Vec<u8>);

/// USB ids of the ports that are instruments. They are never offered as plain
/// serial ports, so nothing but their driver writes to them.
const INSTRUMENTS: &[(u16, u16)] = &[
    otii_arc::device::USB_ID,
    benchwatt_gpp::USB_ID,
    (benchwatt_ppk2::VID, benchwatt_ppk2::PID),
];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Parity {
    #[default]
    None,
    Even,
    Odd,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Flow {
    #[default]
    None,
    /// Hardware flow control; the driver then owns RTS.
    RtsCts,
    XonXoff,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub baud: u32,
    pub data_bits: u8,
    pub parity: Parity,
    pub stop_bits: u8,
    pub flow: Flow,
    /// The level of the control lines once the port is open. The kernel raises
    /// both while opening, so "off" is a short pulse, not silence.
    pub dtr: bool,
    pub rts: bool,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            baud: 115_200,
            data_bits: 8,
            parity: Parity::None,
            stop_bits: 1,
            flow: Flow::None,
            dtr: true,
            rts: true,
        }
    }
}
impl Settings {
    /// Data bits, parity and stop bits the way a terminal program writes them: `8N1`.
    pub fn framing(&self) -> String {
        let parity = match self.parity {
            Parity::None => 'N',
            Parity::Even => 'E',
            Parity::Odd => 'O',
        };
        format!("{}{parity}{}", self.data_bits, self.stop_bits)
    }
    pub fn set_framing(&mut self, text: &str) -> Result<()> {
        let bad = || Error::invalid(format!("framing must look like 8N1, not {text}"));
        let &[data, parity, stop] = text.as_bytes() else {
            return Err(bad());
        };
        self.data_bits = match data {
            b'5'..=b'8' => data - b'0',
            _ => return Err(bad()),
        };
        self.parity = match parity.to_ascii_uppercase() {
            b'N' => Parity::None,
            b'E' => Parity::Even,
            b'O' => Parity::Odd,
            _ => return Err(bad()),
        };
        self.stop_bits = match stop {
            b'1' | b'2' => stop - b'0',
            _ => return Err(bad()),
        };
        Ok(())
    }
    /// Apply one `key=value` of `flow`, `dtr` or `rts`.
    pub fn set(&mut self, key: &str, value: &str) -> Result<()> {
        let on = || match value {
            "on" | "1" | "true" => Ok(true),
            "off" | "0" | "false" => Ok(false),
            _ => Err(Error::invalid(format!("{key} must be on or off"))),
        };
        match key {
            "flow" => {
                self.flow = match value {
                    "none" => Flow::None,
                    "rtscts" => Flow::RtsCts,
                    "xonxoff" => Flow::XonXoff,
                    _ => return Err(Error::invalid("flow must be none, rtscts or xonxoff")),
                }
            }
            "dtr" => self.dtr = on()?,
            "rts" => self.rts = on()?,
            _ => return Err(Error::invalid(format!("unknown serial setting {key}"))),
        }
        Ok(())
    }
    /// `PORT[:BAUD[:FRAMING]]`, e.g. `/dev/ttyUSB1:9600:7E1`.
    pub fn parse(spec: &str) -> Result<(String, Self)> {
        let mut parts = spec.split(':');
        let port = parts.next().filter(|p| !p.is_empty());
        let port = port.ok_or_else(|| Error::invalid("serial port is missing"))?;
        let mut settings = Self::default();
        if let Some(baud) = parts.next() {
            settings.baud = baud
                .parse()
                .ok()
                .filter(|&b| b > 0)
                .ok_or_else(|| Error::invalid(format!("invalid baud rate {baud}")))?;
        }
        if let Some(framing) = parts.next() {
            settings.set_framing(framing)?;
        }
        match parts.next() {
            Some(_) => Err(Error::invalid("serial port is PORT[:BAUD[:FRAMING]]")),
            None => Ok((port.into(), settings)),
        }
    }
    /// How a recording describes its port: `/dev/ttyUSB1 115200 8N1`.
    pub fn describe(&self, port: &str) -> String {
        format!("{port} {} {}", self.baud, self.framing())
    }
}

/// A serial port that can be linked.
#[derive(Clone, Debug, Serialize)]
pub struct PortInfo {
    pub path: String,
    /// What the adapter calls itself, when it says.
    pub name: String,
}
/// The PC's serial ports, without those that are instruments.
pub fn ports() -> Vec<PortInfo> {
    let mut ports: Vec<PortInfo> = serialport::available_ports()
        .unwrap_or_default()
        .into_iter()
        .filter(|p| !is_empty_slot(&p.port_name))
        .filter_map(|p| {
            let name = match p.port_type {
                SerialPortType::UsbPort(u) if INSTRUMENTS.contains(&(u.vid, u.pid)) => return None,
                SerialPortType::UsbPort(u) => u.product.or(u.manufacturer).unwrap_or_default(),
                _ => String::new(),
            };
            Some(PortInfo {
                path: p.port_name,
                name: name.replace('_', " "),
            })
        })
        .collect();
    ports.sort_by(|a, b| a.path.cmp(&b.path));
    ports
}
/// Linux creates `/dev/ttyS0` to `ttyS31` whether or not there is a UART
/// behind them. Those without one report type 0 (unknown).
fn is_empty_slot(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.starts_with("ttyS")
        && std::fs::read_to_string(format!("/sys/class/tty/{name}/type"))
            .is_ok_and(|t| t.trim() == "0")
}
/// Whether this path, however it is spelled, is the port of a supported
/// instrument. Those are never opened as plain ports.
fn is_instrument(path: &str) -> bool {
    let real = |p: &str| std::fs::canonicalize(p).unwrap_or_else(|_| p.into());
    let wanted = real(path);
    serialport::available_ports()
        .unwrap_or_default()
        .into_iter()
        .any(|p| match p.port_type {
            SerialPortType::UsbPort(u) => {
                INSTRUMENTS.contains(&(u.vid, u.pid)) && real(&p.port_name) == wanted
            }
            _ => false,
        })
}
/// The two channels a linked port adds to a recording.
pub fn channels() -> [ChannelDesc; 2] {
    let mut tx = ChannelDesc::new(TX_ID, "serial.tx", "Serial TX", "", 0, Kind::Bytes);
    tx.set = true;
    [
        ChannelDesc::new(RX_ID, "serial.rx", "Serial RX", "", 0, Kind::Bytes),
        tx,
    ]
}

fn io_error(e: impl std::fmt::Display) -> Error {
    Error::new(ErrorKind::Io, e.to_string())
}

/// An open serial port. Received bytes go to the capture it is attached to and
/// are dropped while there is none.
pub struct SerialLink {
    port: Mutex<Box<dyn SerialPort>>,
    sink: Arc<Mutex<Option<Sender<Chunk>>>>,
    closed: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    reader: Option<JoinHandle<()>>,
}
impl SerialLink {
    pub fn open(path: &str, settings: &Settings) -> Result<Self> {
        let data_bits = match settings.data_bits {
            5 => serialport::DataBits::Five,
            6 => serialport::DataBits::Six,
            7 => serialport::DataBits::Seven,
            8 => serialport::DataBits::Eight,
            _ => return Err(Error::invalid("data bits must be 5 to 8")),
        };
        let stop_bits = match settings.stop_bits {
            1 => serialport::StopBits::One,
            2 => serialport::StopBits::Two,
            _ => return Err(Error::invalid("stop bits must be 1 or 2")),
        };
        if settings.baud == 0 {
            return Err(Error::invalid("invalid baud rate"));
        }
        if is_instrument(path) {
            return Err(Error::invalid(format!(
                "{path} is an instrument, not a port to record"
            )));
        }
        let mut port = serialport::new(path, settings.baud)
            .data_bits(data_bits)
            .stop_bits(stop_bits)
            .parity(match settings.parity {
                Parity::None => serialport::Parity::None,
                Parity::Even => serialport::Parity::Even,
                Parity::Odd => serialport::Parity::Odd,
            })
            .flow_control(match settings.flow {
                Flow::None => serialport::FlowControl::None,
                Flow::RtsCts => serialport::FlowControl::Hardware,
                Flow::XonXoff => serialport::FlowControl::Software,
            })
            .timeout(Duration::from_millis(20))
            .open()
            .map_err(|e| io_error(format!("{path}: {e}")))?;
        // A port without modem lines (a pty, some adapters) has nothing to set.
        let _ = port.write_data_terminal_ready(settings.dtr);
        if settings.flow != Flow::RtsCts {
            let _ = port.write_request_to_send(settings.rts);
        }
        let mut input = port.try_clone().map_err(io_error)?;
        let sink: Arc<Mutex<Option<Sender<Chunk>>>> = Arc::default();
        let (closed, stop) = (Arc::<AtomicBool>::default(), Arc::<AtomicBool>::default());
        let reader = {
            let (sink, closed, stop) = (sink.clone(), closed.clone(), stop.clone());
            thread::spawn(move || {
                let mut buf = [0u8; 4096];
                while !stop.load(Ordering::Acquire) {
                    match input.read(&mut buf) {
                        // End of file: the other end of a pty hung up.
                        Ok(0) => break,
                        Ok(n) => {
                            let at = Instant::now();
                            let mut sink = sink.lock().unwrap();
                            if let Some(tx) = sink.as_ref()
                                && tx.send((at, buf[..n].to_vec())).is_err()
                            {
                                *sink = None;
                            }
                        }
                        Err(e)
                            if matches!(
                                e.kind(),
                                io::ErrorKind::TimedOut
                                    | io::ErrorKind::WouldBlock
                                    | io::ErrorKind::Interrupted
                            ) => {}
                        Err(_) => break,
                    }
                }
                closed.store(true, Ordering::Release);
            })
        };
        Ok(Self {
            port: Mutex::new(port),
            sink,
            closed,
            stop,
            reader: Some(reader),
        })
    }
    /// Send what arrives from now on to a new receiver, replacing the previous one.
    pub(crate) fn attach(&self) -> Receiver<Chunk> {
        let (tx, rx) = mpsc::channel();
        *self.sink.lock().unwrap() = Some(tx);
        rx
    }
    pub fn write(&self, bytes: &[u8]) -> Result<()> {
        let mut port = self.port.lock().unwrap();
        port.write_all(bytes).map_err(io_error)?;
        port.flush().map_err(io_error)
    }
    pub fn set_dtr(&self, on: bool) -> Result<()> {
        let mut port = self.port.lock().unwrap();
        port.write_data_terminal_ready(on).map_err(io_error)
    }
    pub fn set_rts(&self, on: bool) -> Result<()> {
        let mut port = self.port.lock().unwrap();
        port.write_request_to_send(on).map_err(io_error)
    }
    /// Drop DTR for a moment and put it back at `level`, which resets a board
    /// that wires DTR to reset.
    pub fn pulse_dtr(&self, level: bool) -> Result<()> {
        self.set_dtr(!level)?;
        thread::sleep(Duration::from_millis(100));
        self.set_dtr(level)
    }
    /// True once the port went away (unplugged) or its reader ended.
    pub fn closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }
}
impl Drop for SerialLink {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn framing_round_trips() {
        let mut s = Settings::default();
        assert_eq!(s.framing(), "8N1");
        s.set_framing("7e2").unwrap();
        assert_eq!((s.data_bits, s.parity, s.stop_bits), (7, Parity::Even, 2));
        assert_eq!(s.framing(), "7E2");
        for bad in ["", "8N", "9N1", "8X1", "8N3", "8N1x"] {
            assert!(s.set_framing(bad).is_err(), "{bad}");
        }
    }
    #[test]
    fn parses_a_port_spec() {
        let (port, s) = Settings::parse("/dev/ttyUSB1").unwrap();
        assert_eq!((port.as_str(), s.baud), ("/dev/ttyUSB1", 115_200));
        let (_, s) = Settings::parse("/dev/ttyUSB1:9600:7O1").unwrap();
        assert_eq!((s.baud, s.framing().as_str()), (9600, "7O1"));
        assert_eq!(s.describe("/dev/ttyUSB1"), "/dev/ttyUSB1 9600 7O1");
        for bad in ["", ":9600", "p:0", "p:x", "p:9600:8N1:z"] {
            assert!(Settings::parse(bad).is_err(), "{bad}");
        }
    }
    #[test]
    fn sets_the_rest_by_key() {
        let mut s = Settings::default();
        s.set("flow", "rtscts").unwrap();
        s.set("dtr", "off").unwrap();
        assert_eq!((s.flow, s.dtr, s.rts), (Flow::RtsCts, false, true));
        assert!(s.set("flow", "both").is_err());
        assert!(s.set("dtr", "maybe").is_err());
        assert!(s.set("speed", "1").is_err());
    }
}
