//! GW Instek GPP bench supplies (GPP-1326, -2323, -3323, -4323) over their USB
//! serial bridge or the LAN socket, behind the traits of `benchwatt-device`.
use benchwatt_device::{
    Block, ChannelDesc, Data, Device, DeviceInfo, Error, ErrorKind, Event, Kind, Meter, OutputDesc,
    OutputState, Range, Result, SettingDesc, SettingKind, Supply, Value,
};
use std::{
    io::{self, Read, Write},
    net::{TcpStream, ToSocketAddrs},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const TIMEOUT: Duration = Duration::from_millis(500);
/// Consecutive polls failing on I/O after which the device counts as gone.
const MAX_FAILURES: u32 = 3;
const RATES: (f64, f64) = (1.0, 50.0);
const BAUD: u32 = 115_200;
const LAN_PORT: u16 = 1026;

// The command set. Queries end in `?` and get one reply line; nothing else gets one.
const IDN: &str = "*IDN?";
const ERROR: &str = ":SYSTem:ERRor?";
const MEASURE_V: &str = ":MEASure:VOLTage:ALL?";
const MEASURE_I: &str = ":MEASure:CURRent:ALL?";
const LOCAL: &str = "LOCAL";
fn voltage(n: u8) -> String {
    format!(":SOURce{n}:VOLTage")
}
fn current(n: u8) -> String {
    format!(":SOURce{n}:CURRent")
}
fn enable(n: u8) -> String {
    format!(":OUTPut{n}:STATe")
}
fn protection(n: u8, what: &str) -> String {
    format!(":OUTPut{n}:{what}")
}

#[derive(Clone, Debug, PartialEq)]
struct Output {
    voltage: Range,
    /// Not empty: the only voltages it takes.
    steps: &'static [f64],
    /// `None` when the limit cannot be set.
    current: Option<Range>,
    measured: bool,
    /// Highest OVP and OCP setting; the unit itself checks them too.
    ovp: f64,
    ocp: f64,
}
const fn output(volts: f64, amps: f64, ovp: f64, ocp: f64) -> Output {
    Output {
        voltage: Range {
            min: 0.0,
            max: volts,
        },
        steps: &[],
        current: Some(Range {
            min: 0.0,
            max: amps,
        }),
        measured: true,
        ovp,
        ocp,
    }
}
const MAIN: Output = output(32.0, 3.0, 35.0, 3.5);
const FIXED: Output = Output {
    voltage: Range { min: 1.8, max: 5.0 },
    steps: &[1.8, 2.5, 3.3, 5.0],
    current: None,
    measured: false,
    ovp: 0.0,
    ocp: 0.0,
};
fn model(name: &str) -> Option<Vec<Output>> {
    Some(match name {
        "GPP-1326" => vec![output(32.0, 6.0, 35.0, 6.5)],
        "GPP-2323" => vec![MAIN; 2],
        "GPP-3323" => vec![MAIN, MAIN, FIXED],
        // CH3 and CH4 take a little more than their nominal 5 V, 15 V and 1 A.
        "GPP-4323" => vec![
            MAIN,
            MAIN,
            output(5.5, 1.1, 6.0, 1.2),
            output(16.0, 1.1, 16.5, 1.2),
        ],
        _ => return None,
    })
}
/// The colours of the channel markings on the front panel, where we know them.
fn colors(model: &str) -> &'static [&'static str] {
    match model {
        "GPP-4323" => &["yellow", "cyan", "magenta", "green"],
        _ => &[],
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
struct Identity {
    model: String,
    serial: String,
    firmware: String,
}
fn identity(reply: &str) -> Result<Identity> {
    let mut fields = reply.split(',').map(str::trim);
    let maker = fields.next().unwrap_or_default();
    let model = fields.next().unwrap_or_default().to_ascii_uppercase();
    if !maker.replace(' ', "").eq_ignore_ascii_case("GWINSTEK") {
        return Err(Error::invalid(format!("not a GW Instek device: {reply}")));
    }
    if !model.starts_with("GPP-") {
        return Err(Error::invalid(format!("not a GPP supply: {reply}")));
    }
    let serial = fields.next().unwrap_or_default();
    let serial = match serial.get(..3) {
        Some(p) if p.eq_ignore_ascii_case("SN:") => serial[3..].trim(),
        _ => serial,
    };
    Ok(Identity {
        model,
        serial: serial.into(),
        firmware: fields.next().unwrap_or_default().into(),
    })
}
fn unexpected(reply: &str) -> Error {
    Error::new(ErrorKind::Io, format!("unexpected reply {reply:?}"))
}
fn number(reply: &str) -> Result<f64> {
    reply.trim().parse().map_err(|_| unexpected(reply))
}
fn numbers(reply: &str) -> Option<Vec<f64>> {
    reply.split(',').map(|s| s.trim().parse().ok()).collect()
}
fn on_off(reply: &str) -> Result<bool> {
    match reply.trim().to_ascii_uppercase().as_str() {
        "ON" | "1" => Ok(true),
        "OFF" | "0" => Ok(false),
        _ => Err(unexpected(reply)),
    }
}
fn no_error(reply: &str) -> bool {
    reply.to_ascii_lowercase().contains("no error")
}

#[derive(Debug, PartialEq)]
enum Target {
    Serial(String, u32),
    Tcp(String),
}
fn target(port: &str) -> Result<Target> {
    if let Some(addr) = port.strip_prefix("tcp:") {
        return Ok(Target::Tcp(match addr.rsplit_once(':') {
            Some((_, p)) if p.parse::<u16>().is_ok() => addr.into(),
            _ => format!("{addr}:{LAN_PORT}"),
        }));
    }
    Ok(match port.rsplit_once('@') {
        Some((path, baud)) => Target::Serial(
            path.into(),
            baud.parse()
                .map_err(|_| Error::invalid(format!("bad baud rate {baud}")))?,
        ),
        None => Target::Serial(port.into(), BAUD),
    })
}

const V: u16 = 1;
const I: u16 = 2;
const P: u16 = 3;
fn ch(n: u8, q: u16) -> u16 {
    u16::from(n) * 10 + q
}
/// The channel values of one poll; power is V × I because the unit's own
/// power readback has 10 mW steps.
fn readings(outputs: &[Output], vs: &[f64], is: &[f64]) -> Vec<(u16, f32)> {
    let mut values = Vec::new();
    for ((o, n), &v) in outputs.iter().zip(1u8..).zip(vs) {
        values.push((ch(n, V), v as f32));
        if let (true, Some(&i)) = (o.measured, is.get(usize::from(n) - 1)) {
            values.push((ch(n, I), i as f32));
            values.push((ch(n, P), (v * i) as f32));
        }
    }
    values
}

pub fn find() -> Vec<String> {
    serialport::available_ports()
        .unwrap_or_default()
        .into_iter()
        .filter(|p| {
            matches!(&p.port_type, serialport::SerialPortType::UsbPort(u)
                if u.vid == 0x2184 && u.pid == 0x0057)
        })
        .map(|p| p.port_name)
        .collect()
}

struct Link {
    writer: Box<dyn Write + Send>,
    lines: Receiver<io::Result<String>>,
    timeout: Duration,
}
fn io_error(e: io::Error) -> Error {
    Error::new(ErrorKind::Io, e.to_string())
}
impl Link {
    fn send(&mut self, line: &str) -> Result<()> {
        if line.is_empty() {
            return Err(Error::invalid("empty command"));
        }
        self.writer
            .write_all(format!("{line}\n").as_bytes())
            .and_then(|_| self.writer.flush())
            .map_err(io_error)
    }
    /// Throws away replies nobody waits for any more, such as a late one.
    fn drain(&mut self) -> Result<()> {
        loop {
            match self.lines.try_recv() {
                Ok(Ok(_)) => {}
                Ok(Err(e)) => return Err(io_error(e)),
                Err(TryRecvError::Empty) => return Ok(()),
                Err(TryRecvError::Disconnected) => {
                    return Err(Error::new(ErrorKind::Io, "connection closed"));
                }
            }
        }
    }
    fn query(&mut self, line: &str) -> Result<String> {
        self.drain()?;
        self.send(line)?;
        match self.lines.recv_timeout(self.timeout) {
            Ok(Ok(reply)) => Ok(reply),
            Ok(Err(e)) => Err(io_error(e)),
            Err(RecvTimeoutError::Timeout) => Err(Error::new(
                ErrorKind::Timeout,
                format!("no reply to {line}"),
            )),
            Err(RecvTimeoutError::Disconnected) => {
                Err(Error::new(ErrorKind::Io, "connection closed"))
            }
        }
    }
    fn run(&mut self, lines: &[String]) -> Result<Vec<String>> {
        let mut replies = Vec::new();
        for line in lines {
            if line.ends_with('?') {
                replies.push(self.query(line)?);
            } else {
                self.send(line)?;
            }
        }
        Ok(replies)
    }
}
fn spawn_reader<R: Read + Send + 'static>(
    mut reader: R,
    stop: Arc<AtomicBool>,
) -> Receiver<io::Result<String>> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let (mut buf, mut line) = ([0u8; 256], Vec::new());
        while !stop.load(Ordering::Relaxed) {
            match reader.read(&mut buf) {
                Ok(0) => {
                    let _ = tx.send(Err(io::ErrorKind::UnexpectedEof.into()));
                    return;
                }
                Ok(n) => {
                    for &b in &buf[..n] {
                        if b != b'\n' {
                            line.push(b);
                            continue;
                        }
                        let text = String::from_utf8_lossy(&line).trim().to_string();
                        line.clear();
                        if !text.is_empty() && tx.send(Ok(text)).is_err() {
                            return;
                        }
                    }
                }
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::TimedOut
                            | io::ErrorKind::WouldBlock
                            | io::ErrorKind::Interrupted
                    ) => {}
                Err(e) => {
                    let _ = tx.send(Err(e));
                    return;
                }
            }
        }
    });
    rx
}

struct Stream {
    sink: Sender<Block>,
    channels: Vec<u16>,
    first: Option<Instant>,
}
struct Shared {
    rate: Mutex<f64>,
    gone: AtomicBool,
    stream: Mutex<Option<Stream>>,
    subscribers: Mutex<Vec<Sender<Event>>>,
}
impl Shared {
    fn broadcast(&self, event: Event) {
        self.subscribers
            .lock()
            .unwrap()
            .retain(|s| s.send(event.clone()).is_ok());
    }
    fn rate(&self) -> f64 {
        *self.rate.lock().unwrap()
    }
    fn period(&self) -> Duration {
        Duration::from_secs_f64(1.0 / self.rate())
    }
    fn publish(&self, values: &[(u16, f32)], at: Instant) {
        for &(ch, value) in values {
            self.broadcast(Event::Reading { ch, value });
        }
        let dt = self.period().as_secs_f64();
        let mut stream = self.stream.lock().unwrap();
        if let Some(s) = stream.as_mut() {
            let t = (at - *s.first.get_or_insert(at)).as_secs_f64();
            let delivered =
                values
                    .iter()
                    .filter(|(c, _)| s.channels.contains(c))
                    .all(|&(ch, v)| {
                        s.sink
                            .send(Block {
                                ch,
                                t,
                                dt,
                                data: Data::Analog(vec![v]),
                            })
                            .is_ok()
                    });
            if !delivered {
                *stream = None;
            }
        }
    }
    fn lost(&self) {
        self.gone.store(true, Ordering::Relaxed);
        // Dropping the sink tells whoever reads the stream that it is over.
        *self.stream.lock().unwrap() = None;
        self.broadcast(Event::Disconnected);
    }
}

enum Op {
    Run(Vec<String>, Sender<Result<Vec<String>>>),
    Quit,
}
/// One poll; a reply that does not parse is skipped, not an error.
fn poll(link: &mut Link, outputs: &[Output], shared: &Shared) -> Result<()> {
    let v = link.query(MEASURE_V)?;
    let i = link.query(MEASURE_I)?;
    let at = Instant::now();
    // A short list is a stray line, not this poll's reply.
    let full = |r: &str| numbers(r).filter(|x| x.len() >= outputs.len());
    if let (Some(vs), Some(is)) = (full(&v), full(&i)) {
        shared.publish(&readings(outputs, &vs, &is), at);
    }
    Ok(())
}
/// Owns the transport: runs callers' commands one at a time and polls between them.
fn run_io(mut link: Link, ops: Receiver<Op>, shared: Arc<Shared>, outputs: Vec<Output>) {
    let (mut last, mut failures) = (None::<Instant>, 0);
    loop {
        let polling = !shared.gone.load(Ordering::Relaxed);
        let due = last.map(|l| l + shared.period());
        if polling && due.is_none_or(|d| Instant::now() >= d) {
            last = Some(Instant::now());
            match poll(&mut link, &outputs, &shared) {
                Ok(()) => failures = 0,
                Err(e) if e.kind == ErrorKind::Io => {
                    failures += 1;
                    if failures >= MAX_FAILURES {
                        shared.lost();
                    }
                }
                Err(_) => {}
            }
        }
        let wait = match (polling, last) {
            (true, Some(l)) => (l + shared.period()).saturating_duration_since(Instant::now()),
            _ => Duration::from_secs(1),
        };
        match ops.recv_timeout(wait) {
            Ok(Op::Run(lines, reply)) => {
                let _ = reply.send(link.run(&lines));
            }
            Ok(Op::Quit) | Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => {}
        }
    }
    let _ = link.send(LOCAL);
}

pub struct Gpp {
    id: Identity,
    raw: String,
    outputs: Vec<Output>,
    shared: Arc<Shared>,
    ops: Sender<Op>,
    io: Option<JoinHandle<()>>,
    stop: Arc<AtomicBool>,
}
impl Gpp {
    /// `port` is a serial path, optionally `PATH@BAUD`, or `tcp:HOST[:PORT]`.
    pub fn open(port: &str) -> Result<Gpp> {
        let io = |e: io::Error| Error::new(ErrorKind::Io, format!("{port}: {e}"));
        match target(port)? {
            Target::Serial(path, baud) => {
                let serial = serialport::new(&path, baud)
                    .data_bits(serialport::DataBits::Eight)
                    .parity(serialport::Parity::None)
                    .stop_bits(serialport::StopBits::One)
                    .flow_control(serialport::FlowControl::None)
                    .timeout(Duration::from_millis(50))
                    .open()
                    .map_err(|e| Error::new(ErrorKind::Io, format!("{path}: {e}")))?;
                let _ = serial.clear(serialport::ClearBuffer::Input);
                let reader = serial
                    .try_clone()
                    .map_err(|e| Error::new(ErrorKind::Io, format!("{path}: {e}")))?;
                Self::with_io(reader, serial)
            }
            Target::Tcp(addr) => {
                let to = addr
                    .to_socket_addrs()
                    .map_err(io)?
                    .next()
                    .ok_or_else(|| Error::new(ErrorKind::Io, format!("{port}: no address")))?;
                let socket = TcpStream::connect_timeout(&to, Duration::from_secs(3)).map_err(io)?;
                socket
                    .set_read_timeout(Some(Duration::from_millis(50)))
                    .map_err(io)?;
                socket.set_nodelay(true).map_err(io)?;
                Self::with_io(socket.try_clone().map_err(io)?, socket)
            }
        }
    }
    /// Over any byte stream. Reads should time out now and then (tens of
    /// milliseconds) so the reader thread can end after the device is dropped.
    pub fn with_io<R: Read + Send + 'static, W: Write + Send + 'static>(
        reader: R,
        writer: W,
    ) -> Result<Gpp> {
        let stop = Arc::new(AtomicBool::new(false));
        let mut link = Link {
            writer: Box::new(writer),
            lines: spawn_reader(reader, stop.clone()),
            timeout: TIMEOUT,
        };
        let found = match link.query(IDN) {
            Err(e) if e.kind == ErrorKind::Timeout => link.query(IDN),
            r => r,
        }
        .and_then(|raw| Ok((identity(&raw)?, raw)));
        let (id, raw) = match found {
            Ok(x) => x,
            Err(e) => {
                stop.store(true, Ordering::Relaxed);
                return Err(e);
            }
        };
        // Old errors would otherwise fail the first set.
        for _ in 0..8 {
            match link.query(ERROR) {
                Ok(r) if !no_error(&r) => continue,
                _ => break,
            }
        }
        let outputs = model(&id.model).unwrap_or_else(|| {
            eprintln!(
                "benchwatt-gpp: unknown model {}, using CH1 and CH2 at 32 V / 3 A",
                id.model
            );
            vec![MAIN; 2]
        });
        let shared = Arc::new(Shared {
            rate: Mutex::new(10.0),
            gone: AtomicBool::new(false),
            stream: Mutex::new(None),
            subscribers: Mutex::new(Vec::new()),
        });
        let (ops, queue) = mpsc::channel();
        let io = {
            let (shared, outputs) = (shared.clone(), outputs.clone());
            thread::spawn(move || run_io(link, queue, shared, outputs))
        };
        Ok(Gpp {
            id,
            raw,
            outputs,
            shared,
            ops,
            io: Some(io),
            stop,
        })
    }
    fn run(&self, lines: Vec<String>) -> Result<Vec<String>> {
        let closed = || Error::new(ErrorKind::Closed, "device closed");
        let (tx, rx) = mpsc::channel();
        self.ops.send(Op::Run(lines, tx)).map_err(|_| closed())?;
        rx.recv().map_err(|_| closed())?
    }
    fn query(&self, line: String) -> Result<String> {
        Ok(self.run(vec![line])?.remove(0))
    }
    /// Sends a setting and reads the error queue right after it.
    fn command(&self, line: String) -> Result<()> {
        let reply = self.run(vec![line.clone(), ERROR.into()])?.remove(0);
        if !no_error(&reply) {
            return Err(Error::invalid(format!("{line}: {reply}")));
        }
        self.shared.broadcast(Event::Changed);
        Ok(())
    }
    fn output(&self, out: u8) -> Result<&Output> {
        usize::from(out)
            .checked_sub(1)
            .and_then(|k| self.outputs.get(k))
            .ok_or_else(|| Error::invalid(format!("no output {out}")))
    }
    /// The output, header and kind behind an `ovp{n}`, `ovp{n}_on`, `ocp{n}`, `ocp{n}_on` key.
    fn protection(&self, key: &str) -> Result<Option<(&Output, String, bool)>> {
        let (what, rest) = match (key.strip_prefix("ovp"), key.strip_prefix("ocp")) {
            (Some(rest), _) => ("OVP", rest),
            (_, Some(rest)) => ("OCP", rest),
            _ => return Ok(None),
        };
        let (n, state) = match rest.strip_suffix("_on") {
            Some(n) => (n, true),
            None => (rest, false),
        };
        let Ok(n) = n.parse::<u8>() else {
            return Ok(None);
        };
        let o = self.output(n)?;
        if o.current.is_none() {
            return Err(Error::invalid(format!("CH{n} has no {what} setting")));
        }
        let header = match state {
            true => protection(n, &format!("{what}:STATe")),
            false => protection(n, what),
        };
        Ok(Some((o, header, state)))
    }
}
impl Drop for Gpp {
    fn drop(&mut self) {
        let _ = self.ops.send(Op::Quit);
        if let Some(io) = self.io.take() {
            let _ = io.join();
        }
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl Device for Gpp {
    fn info(&self) -> DeviceInfo {
        let mut details = serde_json::Map::new();
        details.insert("identity".into(), self.raw.clone().into());
        DeviceInfo {
            kind: "gpp".into(),
            model: self.id.model.clone(),
            serial: self.id.serial.clone(),
            firmware: self.id.firmware.clone(),
            hardware: String::new(),
            details,
        }
    }
    fn channels(&self) -> Vec<ChannelDesc> {
        let rate = self.shared.rate().round() as u32;
        let mut channels = Vec::new();
        for (o, n) in self.outputs.iter().zip(1u8..) {
            let kinds = [
                (V, "v", "voltage", "V"),
                (I, "i", "current", "A"),
                (P, "p", "power", "W"),
            ];
            let count = if o.measured { 3 } else { 1 };
            for &(q, name, what, unit) in &kinds[..count] {
                channels.push(ChannelDesc {
                    group: format!("ch{n}"),
                    set: !o.measured,
                    ..ChannelDesc::new(
                        ch(n, q),
                        &format!("ch{n}.{name}"),
                        &format!("CH{n} {what}"),
                        unit,
                        rate,
                        Kind::Analog,
                    )
                });
            }
        }
        channels
    }
    fn settings(&self) -> Vec<SettingDesc> {
        let setting = |key: String, label: &str, kind, output| SettingDesc {
            key,
            label: label.into(),
            kind,
            output,
            readonly: false,
        };
        let number = |min, max, unit: &str| SettingKind::Number {
            min,
            max,
            unit: unit.into(),
        };
        let mut settings = vec![setting(
            "poll_rate".into(),
            "Poll rate",
            number(RATES.0, RATES.1, "Hz"),
            None,
        )];
        for (o, n) in self.outputs.iter().zip(1u8..) {
            if o.current.is_none() {
                continue;
            }
            let at = Some(n);
            settings.extend([
                setting(
                    format!("ovp{n}"),
                    "Overvoltage limit",
                    number(0.1, o.ovp, "V"),
                    at,
                ),
                setting(
                    format!("ovp{n}_on"),
                    "Overvoltage protection",
                    SettingKind::Bool,
                    at,
                ),
                setting(
                    format!("ocp{n}"),
                    "Overcurrent limit",
                    number(0.01, o.ocp, "A"),
                    at,
                ),
                setting(
                    format!("ocp{n}_on"),
                    "Overcurrent protection",
                    SettingKind::Bool,
                    at,
                ),
            ]);
        }
        settings
    }
    fn get(&self, key: &str) -> Result<Value> {
        if key == "poll_rate" {
            return Ok(self.shared.rate().into());
        }
        let Some((_, header, state)) = self.protection(key)? else {
            return Err(Error::invalid(format!("unknown setting {key}")));
        };
        let reply = self.query(format!("{header}?"))?;
        Ok(match state {
            true => on_off(&reply)?.into(),
            false => number(&reply)?.into(),
        })
    }
    fn set(&self, key: &str, value: Value) -> Result<()> {
        if key == "poll_rate" {
            let hz = value
                .as_f64()
                .filter(|x| (RATES.0..=RATES.1).contains(x))
                .ok_or_else(|| Error::invalid("poll_rate takes 1 to 50 Hz"))?;
            *self.shared.rate.lock().unwrap() = hz;
            // Wakes the I/O thread so the new period counts from now.
            self.run(Vec::new())?;
            self.shared.broadcast(Event::Changed);
            return Ok(());
        }
        let Some((o, header, state)) = self.protection(key)? else {
            return Err(Error::invalid(format!("unknown setting {key}")));
        };
        let line = if state {
            let on = value
                .as_bool()
                .ok_or_else(|| Error::invalid(format!("{key} takes true or false")))?;
            format!("{header} {}", if on { "ON" } else { "OFF" })
        } else {
            let (min, max, unit) = match key.starts_with("ovp") {
                true => (0.1, o.ovp, "V"),
                false => (0.01, o.ocp, "A"),
            };
            let x = value
                .as_f64()
                .filter(|x| (min..=max).contains(x))
                .ok_or_else(|| Error::invalid(format!("{key} takes {min} to {max} {unit}")))?;
            match unit {
                "V" => format!("{header} {x:.1}"),
                _ => format!("{header} {x:.2}"),
            }
        };
        self.command(line)
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

impl Supply for Gpp {
    fn outputs(&self) -> Vec<OutputDesc> {
        self.outputs
            .iter()
            .zip(1u8..)
            .map(|(o, n)| OutputDesc {
                n,
                label: format!("CH{n}"),
                group: format!("ch{n}"),
                voltage: o.voltage,
                voltage_steps: o.steps.to_vec(),
                current_limit: o.current,
                color: colors(&self.id.model)
                    .get(usize::from(n) - 1)
                    .map(|c| (*c).into()),
            })
            .collect()
    }
    fn set_voltage(&self, out: u8, volts: f64) -> Result<()> {
        let o = self.output(out)?;
        let ok = match o.steps {
            [] => o.voltage.contains(volts),
            steps => steps.iter().any(|s| (s - volts).abs() < 1e-6),
        };
        if !ok {
            return Err(Error::invalid(match o.steps {
                [] => format!(
                    "CH{out} takes {} to {} V, not {volts}",
                    o.voltage.min, o.voltage.max
                ),
                steps => format!("CH{out} takes one of {steps:?} V, not {volts}"),
            }));
        }
        self.command(format!("{} {volts:.3}", voltage(out)))
    }
    fn set_current_limit(&self, out: u8, amps: f64) -> Result<()> {
        let o = self.output(out)?;
        let Some(range) = o.current else {
            return Err(Error::invalid(format!("CH{out} has no current limit")));
        };
        if !range.contains(amps) {
            return Err(Error::invalid(format!(
                "CH{out} takes {} to {} A, not {amps}",
                range.min, range.max
            )));
        }
        self.command(format!("{} {amps:.4}", current(out)))
    }
    fn set_enabled(&self, out: u8, on: bool) -> Result<()> {
        self.output(out)?;
        self.command(format!("{} {}", enable(out), if on { "ON" } else { "OFF" }))
    }
    fn state(&self, out: u8) -> Result<OutputState> {
        let o = self.output(out)?;
        let mut lines = vec![format!("{}?", voltage(out)), format!("{}?", enable(out))];
        if o.current.is_some() {
            lines.push(format!("{}?", current(out)));
        }
        let r = self.run(lines)?;
        Ok(OutputState {
            voltage: Some(number(&r[0])?),
            enabled: on_off(&r[1])?,
            current_limit: r.get(2).map(|s| number(s)).transpose()?,
        })
    }
}

impl Meter for Gpp {
    fn start(&self, channels: &[u16], sink: Sender<Block>) -> Result<()> {
        if self.shared.gone.load(Ordering::Relaxed) {
            return Err(Error::new(ErrorKind::Closed, "device disconnected"));
        }
        let known: Vec<u16> = self.channels().iter().map(|c| c.id).collect();
        if let Some(c) = channels.iter().find(|c| !known.contains(c)) {
            return Err(Error::invalid(format!("no channel {c}")));
        }
        *self.shared.stream.lock().unwrap() = Some(Stream {
            sink,
            channels: channels.to_vec(),
            first: None,
        });
        Ok(())
    }
    fn stop(&self) -> Result<()> {
        *self.shared.stream.lock().unwrap() = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identities() {
        let id = |model: &str, serial: &str, firmware: &str| Identity {
            model: model.into(),
            serial: serial.into(),
            firmware: firmware.into(),
        };
        let parse = |s| identity(s).unwrap();
        assert_eq!(
            parse("GW INSTEK,GPP-4323,SN:GEV810395,V1.16"),
            id("GPP-4323", "GEV810395", "V1.16")
        );
        assert_eq!(
            parse("GW INSTEK, GPP-4323, SN:GEV810395, V1.16"),
            id("GPP-4323", "GEV810395", "V1.16")
        );
        assert_eq!(
            parse("GW INSTEK,GPP-3323,GEX123456,V1.00"),
            id("GPP-3323", "GEX123456", "V1.00")
        );
        assert_eq!(parse("GW Instek,gpp-2323"), id("GPP-2323", "", ""));
        for bad in [
            "RIGOL TECHNOLOGIES,DP832,DP8A1,00.01",
            "GW INSTEK,GPD-4303S,X,V1",
            "",
        ] {
            assert_eq!(identity(bad).unwrap_err().kind, ErrorKind::Invalid, "{bad}");
        }
    }

    #[test]
    fn targets() {
        let serial = |p: &str, b| Target::Serial(p.into(), b);
        assert_eq!(
            target("/dev/ttyUSB0").unwrap(),
            serial("/dev/ttyUSB0", 115200)
        );
        assert_eq!(
            target("/dev/ttyUSB0@9600").unwrap(),
            serial("/dev/ttyUSB0", 9600)
        );
        assert!(target("/dev/ttyUSB0@fast").is_err());
        assert_eq!(
            target("tcp:192.168.1.50:1026").unwrap(),
            Target::Tcp("192.168.1.50:1026".into())
        );
        assert_eq!(
            target("tcp:gpp.local").unwrap(),
            Target::Tcp("gpp.local:1026".into())
        );
    }

    #[test]
    fn replies() {
        assert_eq!(
            numbers("5.0012,0.0000,0.0000,0.0000"),
            Some(vec![5.0012, 0.0, 0.0, 0.0])
        );
        assert_eq!(numbers("5.0,x"), None);
        assert_eq!(number("05.000").unwrap(), 5.0);
        assert!(on_off("ON").unwrap());
        assert!(!on_off("OFF").unwrap());
        assert!(on_off("maybe").is_err());
        assert!(no_error("No Error"));
        assert!(!no_error("Undefined Header"));
    }

    #[test]
    fn readings_skip_unmeasured_current() {
        let outputs = model("GPP-3323").unwrap();
        let values = readings(&outputs, &[5.0, 12.0, 3.3], &[0.1, 0.5, 0.0]);
        let ids: Vec<u16> = values.iter().map(|v| v.0).collect();
        assert_eq!(ids, [11, 12, 13, 21, 22, 23, 31]);
        assert_eq!(values[5].1, 6.0);
        assert_eq!(values[6].1, 3.3);
    }
}
