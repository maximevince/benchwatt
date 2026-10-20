use crate::{
    frame,
    msg::{self, Message, UpdateValue},
    prop::{self, ChannelTable, PropertyValue},
};
use std::{
    collections::HashMap,
    io::{Read, Write},
    sync::{
        Arc as StdArc, Mutex,
        mpsc::{self, Receiver, Sender},
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[derive(Debug)]
pub enum Error {
    Serial(String),
    Timeout,
    Protocol(String),
    Status(i32),
    Property(String),
    Closed,
}
impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self::Serial(e.to_string())
    }
}
#[derive(Debug, Clone)]
pub enum Event {
    Monitor { ch: u32, value: f32 },
    Gpi(u32),
    Property { id: u32, raw: Vec<u8> },
    FourWire(u32),
    Alert { id: u16, name: &'static str },
    Data { ms: u32, block: msg::Block },
    Gap { from: u32, to: u32 },
}
#[derive(Debug, Clone, Default)]
pub struct Info {
    pub proto: u32,
    pub max_len: u32,
    pub devtype: u32,
    pub featurebits: u32,
    pub properties: HashMap<u32, PropertyValue>,
    pub chinfo: ChannelTable,
}
type Response = (i32, Vec<u8>);
struct Inner {
    writer: Mutex<Box<dyn Write + Send>>,
    next: Mutex<u32>,
    pending: Mutex<HashMap<u32, Sender<Response>>>,
    subscribers: Mutex<Vec<Sender<Event>>>,
    info: Mutex<Info>,
    recording: Mutex<Vec<u32>>,
}
pub struct Arc {
    inner: StdArc<Inner>,
    stop_reader: Sender<()>,
    reader: Option<thread::JoinHandle<()>>,
    stop_ping: Option<Sender<()>>,
    ping: Option<thread::JoinHandle<()>>,
}
fn broadcast(inner: &Inner, event: Event) {
    inner
        .subscribers
        .lock()
        .unwrap()
        .retain(|s| s.send(event.clone()).is_ok());
}
fn alert_name(id: u16) -> &'static str {
    match id {
        1 => "overtemp",
        2 => "vbus-undervolt",
        3 => "vdc-overvolt",
        4 => "voltcal-fail",
        5 => "main-overvolt",
        6 => "main-overcurr",
        7 => "loadsw-overload",
        8 => "dcdc-overload",
        9 => "prereg-overload",
        10 => "sink-overload",
        11 => "negative-intres",
        12 => "bad-firmware",
        _ => "unknown",
    }
}
impl Arc {
    pub fn open(path: &str) -> Result<Self, Error> {
        let port = serialport::new(path, 115200)
            .timeout(Duration::from_millis(100))
            .open()
            .map_err(|e| Error::Serial(e.to_string()))?;
        let reader = port.try_clone().map_err(|e| Error::Serial(e.to_string()))?;
        let mut arc = Self::from_io(reader, port);
        let deadline = Instant::now() + Duration::from_secs(5);
        let hello = loop {
            match arc.command_with_timeout(0x14, &[], Duration::from_millis(500)) {
                Ok(body) => break body,
                Err(Error::Timeout) if Instant::now() < deadline => continue,
                Err(e) => return Err(e),
            }
        };
        if hello.len() < 16 {
            return Err(Error::Protocol("short hello".into()));
        }
        {
            let mut info = arc.inner.info.lock().unwrap();
            info.proto = u32::from_le_bytes(hello[0..4].try_into().unwrap());
            info.max_len = u32::from_le_bytes(hello[4..8].try_into().unwrap());
            info.devtype = u32::from_le_bytes(hello[8..12].try_into().unwrap());
            info.featurebits = u32::from_le_bytes(hello[12..16].try_into().unwrap());
        }
        let _ = arc.command(0x7e, &[]);
        let (stop_tx, stop_rx) = mpsc::channel();
        let inner = arc.inner.clone();
        arc.ping = Some(thread::spawn(move || {
            while stop_rx.try_recv().is_err() {
                let mut data = unix_micros().to_le_bytes().to_vec();
                let _ = send_ping(&inner, &mut data);
                if stop_rx.recv_timeout(Duration::from_secs(1)).is_ok() {
                    break;
                }
            }
        }));
        arc.stop_ping = Some(stop_tx);
        for id in [1, 0x80, 0x81, 0x82, 0x8d, 9, 11, 12, 6, 10, 0x33, 0x80b] {
            match arc.get(id) {
                Ok(raw) => {
                    let mut info = arc.inner.info.lock().unwrap();
                    if id == 0x8d {
                        info.chinfo = ChannelTable::decode(&raw);
                    } else {
                        info.properties.insert(id, prop::decode(id, &raw));
                    }
                }
                Err(Error::Status(-3)) => {}
                Err(e) => return Err(e),
            }
        }
        Ok(arc)
    }
    /// Construct a protocol session over an existing byte stream (useful for mock transports).
    pub fn from_io<R: Read + Send + 'static, W: Write + Send + 'static>(
        mut reader: R,
        writer: W,
    ) -> Self {
        let (stop_tx, stop_rx) = mpsc::channel();
        let inner = StdArc::new(Inner {
            writer: Mutex::new(Box::new(writer)),
            next: Mutex::new(0),
            pending: Mutex::new(HashMap::new()),
            subscribers: Mutex::new(Vec::new()),
            info: Mutex::new(Info::default()),
            recording: Mutex::new(Vec::new()),
        });
        let shared = inner.clone();
        let worker = thread::spawn(move || {
            let mut decoder = frame::Decoder::new();
            let mut buf = [0u8; 4096];
            let mut last: Option<u32> = None;
            while stop_rx.try_recv().is_err() {
                let n = match reader.read(&mut buf) {
                    Ok(n) => n,
                    Err(e)
                        if e.kind() == std::io::ErrorKind::TimedOut
                            || e.kind() == std::io::ErrorKind::WouldBlock =>
                    {
                        0
                    }
                    Err(_) => break,
                };
                if n == 0 {
                    continue;
                }
                for payload in decoder.push(&buf[..n]) {
                    let table = shared.info.lock().unwrap().chinfo.clone();
                    let Ok(message) = msg::parse(&payload, &table) else {
                        continue;
                    };
                    match message {
                        Message::CmdResp { txn, status, body } => {
                            if let Some(tx) = shared.pending.lock().unwrap().remove(&txn) {
                                let _ = tx.send((status, body));
                            }
                        }
                        Message::Data { ms, blocks } => {
                            if let Some(previous) = last
                                && ms != previous.wrapping_add(1)
                            {
                                broadcast(
                                    &shared,
                                    Event::Gap {
                                        from: previous,
                                        to: ms,
                                    },
                                );
                            }
                            last = Some(ms);
                            for block in blocks {
                                broadcast(&shared, Event::Data { ms, block });
                            }
                        }
                        Message::Update { records } => {
                            for record in records {
                                match record.value {
                                    UpdateValue::Property(_) => broadcast(
                                        &shared,
                                        Event::Property {
                                            id: record.id,
                                            raw: record.raw.clone(),
                                        },
                                    ),
                                    UpdateValue::Monitor(value) => broadcast(
                                        &shared,
                                        Event::Monitor {
                                            ch: record.id,
                                            value,
                                        },
                                    ),
                                    UpdateValue::Gpi(mask) => broadcast(&shared, Event::Gpi(mask)),
                                    UpdateValue::FourWire(value) => {
                                        broadcast(&shared, Event::FourWire(value))
                                    }
                                    UpdateValue::Raw(raw) => {
                                        broadcast(&shared, Event::Property { id: record.id, raw })
                                    }
                                }
                            }
                        }
                        Message::Alert { ids } => {
                            for id in ids {
                                broadcast(
                                    &shared,
                                    Event::Alert {
                                        id,
                                        name: alert_name(id),
                                    },
                                );
                            }
                        }
                        Message::Unknown(_) => {}
                    }
                }
            }
        });
        Self {
            inner,
            stop_reader: stop_tx,
            reader: Some(worker),
            stop_ping: None,
            ping: None,
        }
    }
    fn command_with_timeout(
        &self,
        cmd: u32,
        data: &[u8],
        timeout: Duration,
    ) -> Result<Vec<u8>, Error> {
        let (tx, rx) = mpsc::channel();
        let txn = {
            let mut next = self.inner.next.lock().unwrap();
            let txn = *next;
            *next = next.wrapping_add(1);
            txn
        };
        self.inner.pending.lock().unwrap().insert(txn, tx);
        let mut payload = Vec::with_capacity(8 + data.len());
        payload.extend_from_slice(&txn.to_le_bytes());
        payload.extend_from_slice(&cmd.to_le_bytes());
        payload.extend_from_slice(data);
        let write_result = self
            .inner
            .writer
            .lock()
            .unwrap()
            .write_all(&frame::encode(&payload));
        if let Err(e) = write_result {
            self.inner.pending.lock().unwrap().remove(&txn);
            return Err(e.into());
        }
        let result = rx.recv_timeout(timeout);
        self.inner.pending.lock().unwrap().remove(&txn);
        match result {
            Ok((0, body)) => Ok(body),
            Ok((status, _)) => Err(Error::Status(status)),
            Err(mpsc::RecvTimeoutError::Timeout) => Err(Error::Timeout),
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(Error::Closed),
        }
    }
    fn command(&self, cmd: u32, data: &[u8]) -> Result<Vec<u8>, Error> {
        self.command_with_timeout(cmd, data, Duration::from_secs(1))
    }
    pub fn info(&self) -> Info {
        self.inner.info.lock().unwrap().clone()
    }
    pub fn get(&self, id: u32) -> Result<Vec<u8>, Error> {
        let mut data = id.to_le_bytes().to_vec();
        data.extend_from_slice(&0u32.to_le_bytes());
        self.command(0x64, &data)
    }
    pub fn get_value(&self, id: u32) -> Result<PropertyValue, Error> {
        Ok(prop::decode(id, &self.get(id)?))
    }
    pub fn set(&self, id: u32, value: &PropertyValue) -> Result<(), Error> {
        let data = prop::encode_set(id, value).map_err(Error::Property)?;
        self.command(0x66, &data).map(|_| ())
    }
    pub fn output(&self, on: bool) -> Result<(), Error> {
        self.set(9, &PropertyValue::Bool(on))
    }
    pub fn set_voltage(&self, volts: f64) -> Result<(), Error> {
        self.set(11, &PropertyValue::Number(volts))
    }
    pub fn set_current_limit(&self, amps: f64) -> Result<(), Error> {
        self.set(12, &PropertyValue::Number(amps))
    }
    pub fn enable_channel(&self, ch: u32, on: bool) -> Result<(), Error> {
        let mut data = ch.to_le_bytes().to_vec();
        data.extend_from_slice(&u32::from(on).to_le_bytes());
        self.command(0x78, &data).map(|_| ())
    }
    pub fn start_recording(&self, channels: &[u32]) -> Result<(), Error> {
        if !self.inner.recording.lock().unwrap().is_empty() {
            return Err(Error::Protocol("recording already active".into()));
        }
        let mut enabled = Vec::new();
        for &ch in channels {
            if let Err(e) = self.enable_channel(ch, true) {
                for previous in enabled {
                    let _ = self.enable_channel(previous, false);
                }
                return Err(e);
            }
            enabled.push(ch);
        }
        match self.command(0x7c, &[]) {
            Ok(_) => {
                *self.inner.recording.lock().unwrap() = channels.to_vec();
                Ok(())
            }
            Err(e) => {
                for &ch in channels {
                    let _ = self.enable_channel(ch, false);
                }
                Err(e)
            }
        }
    }
    pub fn stop_recording(&self) -> Result<(), Error> {
        let channels = self.inner.recording.lock().unwrap().clone();
        let stop = self.command(0x7e, &[]).map(|_| ());
        let mut disable = Ok(());
        for ch in channels {
            if let Err(e) = self.enable_channel(ch, false)
                && disable.is_ok()
            {
                disable = Err(e);
            }
        }
        self.inner.recording.lock().unwrap().clear();
        stop.and(disable)
    }
    pub fn uart_open(&self, baud: u32) -> Result<(), Error> {
        self.set(
            0x29,
            &PropertyValue::Int(
                i32::try_from(baud).map_err(|_| Error::Property("baud out of range".into()))?,
            ),
        )?;
        self.set(0x28, &PropertyValue::Bool(true))?;
        self.enable_channel(0x17, true)
    }
    pub fn uart_close(&self) -> Result<(), Error> {
        self.enable_channel(0x17, false)?;
        self.set(0x28, &PropertyValue::Bool(false))
    }
    pub fn uart_write(&self, bytes: &[u8]) -> Result<(), Error> {
        let mut data = 0x19u32.to_le_bytes().to_vec();
        data.extend_from_slice(bytes);
        self.command(0x82, &data).map(|_| ())
    }
    pub fn set_gpo(&self, n: u8, on: bool) -> Result<(), Error> {
        if !(1..=3).contains(&n) {
            return Err(Error::Property("GPO must be 1..=3".into()));
        }
        self.set(
            0x32,
            &PropertyValue::Int(((u32::from(on) + 1) << (3 * (n - 1))) as i32),
        )
    }
    pub fn events(&self) -> Receiver<Event> {
        let (tx, rx) = mpsc::channel();
        self.inner.subscribers.lock().unwrap().push(tx);
        rx
    }
}
fn send_ping(inner: &Inner, data: &mut Vec<u8>) -> Result<(), Error> {
    let txn = {
        let mut next = inner.next.lock().unwrap();
        let txn = *next;
        *next = next.wrapping_add(1);
        txn
    };
    let mut payload = txn.to_le_bytes().to_vec();
    payload.extend_from_slice(&0x0au32.to_le_bytes());
    payload.append(data);
    inner
        .writer
        .lock()
        .unwrap()
        .write_all(&frame::encode(&payload))?;
    Ok(())
}
impl Drop for Arc {
    fn drop(&mut self) {
        if !self.inner.recording.lock().unwrap().is_empty() {
            let _ = self.stop_recording();
        }
        if let Some(tx) = self.stop_ping.take() {
            let _ = tx.send(());
        }
        if let Some(worker) = self.ping.take() {
            let _ = worker.join();
        }
        let _ = self.stop_reader.send(());
        if let Some(worker) = self.reader.take() {
            let _ = worker.join();
        }
    }
}
pub fn unix_micros() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros() as u64
}
pub fn find_port() -> Option<String> {
    serialport::available_ports()
        .ok()?
        .into_iter()
        .find(|p| match &p.port_type {
            serialport::SerialPortType::UsbPort(info) => info.vid == 0x0fce && info.pid == 0xd1e6,
            _ => false,
        })
        .map(|p| p.port_name)
}
