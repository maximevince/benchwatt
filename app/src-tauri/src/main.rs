use benchwatt_session::{
    Capture, Found,
    device::{
        ChannelDesc, Device, Error, ErrorKind, Event, Kind, OutputDesc, OutputState, SettingDesc,
        SettingKind, Severity, Supply, Value,
    },
    rec::{self, Recording, View, export, file},
    serial::{self, SerialLink},
};
use serde::Serialize;
use std::{
    collections::{BTreeMap, HashMap},
    fmt::Display,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, Sender},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use tauri::{AppHandle, Emitter, Manager};

/// Lock order: control → Open::dev → recordings → serial → devices → Open::listed → threads.
/// Only device calls may block while one is held.
struct App {
    /// Serialises starting and stopping recordings and choosing the device.
    control: Mutex<()>,
    recordings: Mutex<Recs>,
    devices: Mutex<Devices>,
    serial: Mutex<Linked>,
    threads: Mutex<Vec<JoinHandle<()>>>,
    running: AtomicBool,
    /// Wakes the scanner when the chosen device changes.
    wake: Mutex<Sender<()>>,
}
/// The serial port recorded next to the device, and how it was last set up.
#[derive(Default)]
struct Linked {
    link: Option<SerialLink>,
    port: Option<String>,
    settings: serial::Settings,
}
impl Linked {
    fn info(&self) -> SerialInfo {
        SerialInfo {
            port: self.port.clone(),
            open: self.link.is_some(),
            settings: self.settings.clone(),
        }
    }
}
#[derive(Clone, Serialize)]
struct SerialInfo {
    port: Option<String>,
    open: bool,
    settings: serial::Settings,
}
/// What is plugged in and the one device the app works with.
#[derive(Default)]
struct Devices {
    found: Vec<Found>,
    /// The device the user chose. It is opened whenever it is plugged in.
    wanted: Option<Found>,
    open: Option<Arc<Open>>,
    /// Why the chosen device could not be opened.
    error: Option<String>,
}
struct Open {
    found: Found,
    id: String,
    dev: Mutex<Box<dyn Device>>,
    listed: Mutex<Listed>,
    uart_open: AtomicBool,
    /// Set when the device went away or is being closed; its worker then ends.
    gone: AtomicBool,
}
/// The open recordings, each a tab in the window. At most one has a capture.
#[derive(Default)]
struct Recs {
    last_id: u32,
    list: BTreeMap<u32, Rec>,
}
struct Rec {
    rec: Recording,
    capture: Option<Capture>,
    /// The device the recording came from, while it is open.
    device: Option<String>,
    meta: file::Meta,
}
impl Recs {
    fn add(&mut self, rec: Rec) -> RecordingInfo {
        self.last_id += 1;
        let info = rec_info(self.last_id, &rec);
        self.list.insert(self.last_id, rec);
        info
    }
    fn get(&self, id: u32) -> Result<&Rec, String> {
        self.list
            .get(&id)
            .ok_or_else(|| "this recording is closed".into())
    }
    /// The recording that is being captured into.
    fn live(&mut self) -> Option<(u32, &mut Rec)> {
        self.list
            .iter_mut()
            .find(|(_, r)| r.capture.is_some())
            .map(|(&id, r)| (id, r))
    }
}

#[derive(Clone, Serialize)]
struct DeviceDesc {
    id: String,
    kind: String,
    model: String,
    serial: String,
    firmware: String,
    hardware: String,
    channels: Vec<ChannelDesc>,
    outputs: Vec<OutputDesc>,
    settings: Vec<SettingDesc>,
    gpo: usize,
    uart: bool,
}
#[derive(Clone, PartialEq, Serialize)]
struct DeviceState {
    outputs: Vec<OutputState>,
    settings: BTreeMap<String, Option<Value>>,
    gpo: Vec<Option<bool>>,
    uart_open: bool,
}
#[derive(Clone, Serialize)]
struct Listed {
    desc: DeviceDesc,
    state: Option<DeviceState>,
}
#[derive(Clone, Serialize)]
struct FoundInfo {
    key: String,
    kind: &'static str,
    name: &'static str,
    port: String,
}
#[derive(Clone, Serialize)]
struct DevicesInfo {
    found: Vec<FoundInfo>,
    wanted: Option<String>,
    active: Option<Listed>,
    error: Option<String>,
}
#[derive(Clone, Serialize)]
struct StateEvent<'a> {
    device: &'a str,
    state: &'a DeviceState,
}
#[derive(Clone, Serialize)]
struct Monitor {
    device: String,
    readings: BTreeMap<String, f32>,
    inputs: Option<u32>,
}
#[derive(Clone, Serialize)]
struct Alert<'a> {
    device: &'a str,
    id: u16,
    name: String,
    title: String,
    detail: String,
    severity: Severity,
}
#[derive(Clone, Serialize)]
struct RecordingInfo {
    id: u32,
    active: bool,
    duration_s: f64,
    samples: HashMap<String, usize>,
    gaps: usize,
    channels: Vec<ChannelDesc>,
    device: Option<String>,
    /// The instrument it was made with, as stored in the file.
    source: file::DeviceInfo,
}
#[derive(Serialize)]
struct UartChunk {
    t: f64,
    bytes: Vec<u8>,
    dir: &'static str,
}

fn text(e: impl Display) -> String {
    e.to_string()
}
fn rec_info(id: u32, r: &Rec) -> RecordingInfo {
    RecordingInfo {
        id,
        active: r.rec.active(),
        duration_s: r.rec.duration(),
        samples: r.rec.sample_counts(),
        gaps: r.rec.gaps().len(),
        channels: r.rec.specs(),
        device: r.device.clone(),
        source: r.meta.device.clone(),
    }
}
/// A device's id: its kind and where it is plugged in.
fn key(found: &Found) -> String {
    format!("{}@{}", found.kind, found.port)
}
fn listing(app: &AppHandle) -> DevicesInfo {
    let state = app.state::<App>();
    let devices = state.devices.lock().unwrap();
    DevicesInfo {
        found: devices
            .found
            .iter()
            .map(|f| FoundInfo {
                key: key(f),
                kind: f.kind,
                name: f.name(),
                port: f.port.clone(),
            })
            .collect(),
        wanted: devices.wanted.as_ref().map(key),
        active: devices
            .open
            .as_ref()
            .map(|o| o.listed.lock().unwrap().clone()),
        error: devices.error.clone(),
    }
}
fn emit_devices(app: &AppHandle) {
    let _ = app.emit("devices", listing(app));
}
fn emit_state(app: &AppHandle, id: &str, state: &DeviceState) {
    let _ = app.emit("state", StateEvent { device: id, state });
}
fn find_open(app: &AppHandle, id: &str) -> Result<Arc<Open>, String> {
    let state = app.state::<App>();
    let devices = state.devices.lock().unwrap();
    devices
        .open
        .iter()
        .find(|o| o.id == id)
        .cloned()
        .ok_or_else(|| format!("device {id} is not connected"))
}

/// Read the outputs (whose ranges can change) and the state. A setting that
/// cannot be read is `None`; a closed device is an error.
fn read_state(
    dev: &dyn Device,
    settings: &[SettingDesc],
    uart_open: bool,
) -> Result<(Vec<OutputDesc>, DeviceState), Error> {
    let (outputs, states) = match dev.supply() {
        Some(s) => {
            let outputs = s.outputs();
            let states = outputs
                .iter()
                .map(|o| s.state(o.n))
                .collect::<Result<_, _>>()?;
            (outputs, states)
        }
        None => (Vec::new(), Vec::new()),
    };
    let mut values = BTreeMap::new();
    for s in settings {
        let value = match dev.get(&s.key) {
            Ok(v) => Some(v),
            Err(e) if e.kind == ErrorKind::Closed => return Err(e),
            Err(_) => None,
        };
        values.insert(s.key.clone(), value);
    }
    Ok((
        outputs,
        DeviceState {
            outputs: states,
            settings: values,
            gpo: dev.gpio().map_or(Vec::new(), |g| g.outputs()),
            uart_open,
        },
    ))
}
/// Read the state again and tell the frontend what changed.
fn refresh(
    app: &AppHandle,
    open: &Open,
    dev: &dyn Device,
    force: bool,
) -> Result<DeviceState, Error> {
    let settings = open.listed.lock().unwrap().desc.settings.clone();
    let (outputs, state) = read_state(dev, &settings, open.uart_open.load(Ordering::Acquire))?;
    let (moved, changed) = {
        let mut listed = open.listed.lock().unwrap();
        let moved =
            serde_json::to_value(&listed.desc.outputs).ok() != serde_json::to_value(&outputs).ok();
        if moved {
            listed.desc.outputs = outputs;
        }
        let changed = listed.state.as_ref() != Some(&state);
        listed.state = Some(state.clone());
        (moved, changed)
    };
    if moved {
        emit_devices(app);
    }
    if changed || force {
        emit_state(app, &open.id, &state);
    }
    Ok(state)
}
fn set_uart_flag(app: &AppHandle, open: &Open, on: bool) {
    open.uart_open.store(on, Ordering::Release);
    let state = open.listed.lock().unwrap().state.as_mut().and_then(|s| {
        let changed = s.uart_open != on;
        s.uart_open = on;
        changed.then(|| s.clone())
    });
    if let Some(state) = state {
        emit_state(app, &open.id, &state);
    }
}

#[tauri::command(async)]
fn get_devices(app: AppHandle) -> DevicesInfo {
    listing(&app)
}
/// Choose the device to work with, or none. The scanner closes the previous
/// one and opens this one; the frontend hears about it through `devices`.
#[tauri::command(async)]
fn select_device(app: AppHandle, device: Option<String>) -> Result<(), String> {
    let state = app.state::<App>();
    let _control = state.control.lock().unwrap();
    if state.recordings.lock().unwrap().live().is_some() {
        return Err("stop the recording before changing the device".into());
    }
    {
        let mut devices = state.devices.lock().unwrap();
        let wanted = match device {
            Some(id) => Some(
                devices
                    .found
                    .iter()
                    .find(|f| key(f) == id)
                    .cloned()
                    .ok_or_else(|| format!("device {id} is not plugged in"))?,
            ),
            None => None,
        };
        devices.wanted = wanted;
        devices.error = None;
    }
    emit_devices(&app);
    let _ = state.wake.lock().unwrap().send(());
    Ok(())
}
/// Run `op` on the device, then return its fresh state.
fn apply(
    app: &AppHandle,
    device: &str,
    op: impl FnOnce(&Open, &dyn Device) -> Result<(), String>,
) -> Result<DeviceState, String> {
    let open = find_open(app, device)?;
    let dev = open.dev.lock().unwrap();
    op(&open, dev.as_ref())?;
    refresh(app, &open, dev.as_ref(), false).map_err(text)
}
fn output(dev: &dyn Device, out: u8) -> Result<(&dyn Supply, OutputDesc), String> {
    let s = dev.supply().ok_or("device has no outputs")?;
    let o = s
        .outputs()
        .into_iter()
        .find(|o| o.n == out)
        .ok_or_else(|| format!("device has no output {out}"))?;
    Ok((s, o))
}
#[tauri::command(async)]
fn set_output(app: AppHandle, device: String, out: u8, on: bool) -> Result<DeviceState, String> {
    apply(&app, &device, |_, dev| {
        output(dev, out)?.0.set_enabled(out, on).map_err(text)
    })
}
#[tauri::command(async)]
fn set_voltage(app: AppHandle, device: String, out: u8, volts: f64) -> Result<DeviceState, String> {
    apply(&app, &device, |_, dev| {
        let (s, o) = output(dev, out)?;
        if !o.voltage.contains(volts) {
            return Err(format!(
                "voltage must be between {} and {} V",
                o.voltage.min, o.voltage.max
            ));
        }
        if !o.voltage_steps.is_empty() && !o.voltage_steps.iter().any(|v| (v - volts).abs() < 1e-6)
        {
            let steps: Vec<String> = o.voltage_steps.iter().map(f64::to_string).collect();
            return Err(format!("voltage must be one of {} V", steps.join(", ")));
        }
        s.set_voltage(out, volts).map_err(text)
    })
}
#[tauri::command(async)]
fn set_current_limit(
    app: AppHandle,
    device: String,
    out: u8,
    amps: f64,
) -> Result<DeviceState, String> {
    apply(&app, &device, |_, dev| {
        let (s, o) = output(dev, out)?;
        let range = o
            .current_limit
            .ok_or_else(|| format!("the current limit of output {out} cannot be set"))?;
        if !range.contains(amps) {
            return Err(format!(
                "current limit must be between {} and {} A",
                range.min, range.max
            ));
        }
        s.set_current_limit(out, amps).map_err(text)
    })
}
#[tauri::command(async)]
fn set_setting(
    app: AppHandle,
    device: String,
    key: String,
    value: Value,
) -> Result<DeviceState, String> {
    apply(&app, &device, |open, dev| {
        let desc = open
            .listed
            .lock()
            .unwrap()
            .desc
            .settings
            .iter()
            .find(|s| s.key == key)
            .cloned();
        let desc = desc.ok_or_else(|| format!("unknown setting {key}"))?;
        let label = if desc.label.is_empty() {
            &key
        } else {
            &desc.label
        };
        if desc.readonly {
            return Err(format!("{label} is read-only"));
        }
        match (&desc.kind, &value) {
            (SettingKind::Bool, Value::Bool(_)) => {}
            (SettingKind::Bool, _) => return Err(format!("{label} must be on or off")),
            (SettingKind::Choice { options }, Value::String(s)) if options.contains(s) => {}
            (SettingKind::Choice { options }, _) => {
                return Err(format!("{label} must be one of {}", options.join(", ")));
            }
            (SettingKind::Number { min, max, .. }, v)
                if v.as_f64()
                    .is_some_and(|x| x.is_finite() && x >= *min && x <= *max) => {}
            (SettingKind::Number { min, max, unit }, _) => {
                let message = format!("{label} must be between {min} and {max} {unit}");
                return Err(message.trim_end().into());
            }
        }
        dev.set(&key, value).map_err(text)
    })
}
#[tauri::command(async)]
fn set_gpo(app: AppHandle, device: String, n: u8, on: bool) -> Result<DeviceState, String> {
    apply(&app, &device, |_, dev| {
        let g = dev.gpio().ok_or("device has no digital outputs")?;
        let count = g.outputs().len();
        if n == 0 || usize::from(n) > count {
            return Err(format!("digital output must be between 1 and {count}"));
        }
        g.set_output(n, on).map_err(text)
    })
}

fn emit_serial(app: &AppHandle) {
    let info = app.state::<App>().serial.lock().unwrap().info();
    let _ = app.emit("serial", info);
}
#[tauri::command(async)]
fn serial_ports() -> Vec<serial::PortInfo> {
    serial::ports()
}
#[tauri::command(async)]
fn serial_info(app: AppHandle) -> SerialInfo {
    app.state::<App>().serial.lock().unwrap().info()
}
/// Open a serial port of the PC next to the device. A running recording
/// starts taking its text from it.
#[tauri::command(async)]
fn serial_open(
    app: AppHandle,
    device: String,
    port: String,
    settings: serial::Settings,
) -> Result<(), String> {
    let state = app.state::<App>();
    let _control = state.control.lock().unwrap();
    let open = find_open(&app, &device)?;
    if open.uart_open.load(Ordering::Acquire) {
        return Err("close the device's UART first: a recording has one source of text".into());
    }
    // Let go of the previous port before opening, in case it is the same one.
    state.serial.lock().unwrap().link = None;
    let link = SerialLink::open(&port, &settings).map_err(text)?;
    {
        let mut recs = state.recordings.lock().unwrap();
        if let Some((_, r)) = recs.live()
            && let Some(capture) = r.capture.as_mut()
        {
            capture.attach(&link, &mut r.rec);
            r.meta.serial = Some(settings.describe(&port));
        }
        *state.serial.lock().unwrap() = Linked {
            link: Some(link),
            port: Some(port),
            settings,
        };
    }
    emit_serial(&app);
    Ok(())
}
#[tauri::command(async)]
fn serial_close(app: AppHandle) {
    app.state::<App>().serial.lock().unwrap().link = None;
    emit_serial(&app);
}
#[tauri::command(async)]
fn serial_send(app: AppHandle, text: String) -> Result<(), String> {
    let state = app.state::<App>();
    let mut recs = state.recordings.lock().unwrap();
    let linked = state.serial.lock().unwrap();
    let link = linked.link.as_ref().ok_or("no serial port is open")?;
    let rec = recs.live().map(|(_, r)| &mut r.rec);
    benchwatt_session::serial_write(link, rec, text.as_bytes()).map_err(|e| e.to_string())
}
/// Set the control lines of the open port; `pulse` drops DTR for a moment to reset a board.
#[tauri::command(async)]
fn serial_lines(
    app: AppHandle,
    dtr: Option<bool>,
    rts: Option<bool>,
    pulse: bool,
) -> Result<(), String> {
    {
        let state = app.state::<App>();
        let mut linked = state.serial.lock().unwrap();
        let linked = &mut *linked;
        let link = linked.link.as_ref().ok_or("no serial port is open")?;
        if let Some(on) = dtr {
            link.set_dtr(on).map_err(text)?;
            linked.settings.dtr = on;
        }
        if let Some(on) = rts {
            link.set_rts(on).map_err(text)?;
            linked.settings.rts = on;
        }
        if pulse {
            link.pulse_dtr(linked.settings.dtr).map_err(text)?;
        }
    }
    emit_serial(&app);
    Ok(())
}
#[tauri::command(async)]
fn uart_open(app: AppHandle, device: String, baud: u32) -> Result<(), String> {
    if baud == 0 || baud > 3_000_000 {
        return Err("invalid baud rate".into());
    }
    if app.state::<App>().serial.lock().unwrap().link.is_some() {
        return Err("close the serial port first: a recording has one source of text".into());
    }
    let open = find_open(&app, &device)?;
    let dev = open.dev.lock().unwrap();
    let uart = dev.uart().ok_or("device has no UART")?;
    uart.open(baud).map_err(text)?;
    {
        let state = app.state::<App>();
        let mut recs = state.recordings.lock().unwrap();
        if let Some((_, r)) = recs.live()
            && r.device.as_deref() == Some(&open.id)
            && !r.rec.specs().iter().any(|c| c.id == uart.rx())
            && let Some(rx) = dev.channels().into_iter().find(|c| c.id == uart.rx())
        {
            r.rec.add_channel(rx);
        }
    }
    set_uart_flag(&app, &open, true);
    Ok(())
}
#[tauri::command(async)]
fn uart_close(app: AppHandle, device: String) -> Result<(), String> {
    let open = find_open(&app, &device)?;
    let dev = open.dev.lock().unwrap();
    dev.uart()
        .ok_or("device has no UART")?
        .close()
        .map_err(text)?;
    set_uart_flag(&app, &open, false);
    Ok(())
}
#[tauri::command(async)]
fn uart_send(app: AppHandle, device: String, text: String) -> Result<(), String> {
    let open = find_open(&app, &device)?;
    let dev = open.dev.lock().unwrap();
    let state = app.state::<App>();
    let mut recs = state.recordings.lock().unwrap();
    let rec = recs
        .live()
        .filter(|(_, r)| r.device.as_deref() == Some(&open.id))
        .map(|(_, r)| &mut r.rec);
    benchwatt_session::uart_write(dev.as_ref(), rec, text.as_bytes()).map_err(|e| e.to_string())
}

/// Stop the running capture, if any, keeping what it delivered. The caller holds `control`.
fn stop_active(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<App>();
    let id = {
        let mut recs = state.recordings.lock().unwrap();
        recs.live().and_then(|(_, r)| r.device.clone())
    };
    // A capture whose device is gone is ended by that device's worker.
    let Some(open) = id.and_then(|id| find_open(app, &id).ok()) else {
        return Ok(());
    };
    let dev = open.dev.lock().unwrap();
    let (result, info) = {
        let mut recs = state.recordings.lock().unwrap();
        let Some((id, r)) = recs.live() else {
            return Ok(());
        };
        let Some(capture) = r.capture.take() else {
            return Ok(());
        };
        (capture.stop(dev.as_ref(), &mut r.rec), rec_info(id, r))
    };
    let _ = app.emit("recording", info);
    result.map_err(text)
}
#[tauri::command(async)]
fn recordings(app: AppHandle) -> Vec<RecordingInfo> {
    let state = app.state::<App>();
    let recs = state.recordings.lock().unwrap();
    recs.list.iter().map(|(&id, r)| rec_info(id, r)).collect()
}
#[tauri::command(async)]
fn start_recording(
    app: AppHandle,
    device: String,
    channels: Vec<String>,
) -> Result<RecordingInfo, String> {
    let state = app.state::<App>();
    let _control = state.control.lock().unwrap();
    let open = find_open(&app, &device)?;
    stop_active(&app)?;
    let dev = open.dev.lock().unwrap();
    let all = dev.channels();
    let mut names: Vec<String> = Vec::new();
    for name in channels {
        if !all.iter().any(|c| c.name == name) {
            return Err(format!("unknown channel {name}"));
        }
        if !names.contains(&name) {
            names.push(name);
        }
    }
    if open.uart_open.load(Ordering::Acquire)
        && let Some(rx) = dev.uart().and_then(|u| all.iter().find(|c| c.id == u.rx()))
        && !names.contains(&rx.name)
    {
        names.push(rx.name.clone());
    }
    if names.is_empty() {
        return Err("select at least one channel".into());
    }
    let (mut capture, mut rec) = Capture::start(dev.as_ref(), &names, false, None).map_err(text)?;
    let mut meta = benchwatt_session::meta(dev.as_ref());
    let mut recs = state.recordings.lock().unwrap();
    {
        let linked = state.serial.lock().unwrap();
        if let (Some(link), Some(port)) = (&linked.link, &linked.port) {
            capture.attach(link, &mut rec);
            meta.serial = Some(linked.settings.describe(port));
        }
    }
    if open.gone.load(Ordering::SeqCst) {
        return Err(format!("device {device} is not connected"));
    }
    Ok(recs.add(Rec {
        rec,
        capture: Some(capture),
        device: Some(device),
        meta,
    }))
}
#[tauri::command(async)]
fn stop_recording(app: AppHandle) -> Result<(), String> {
    let state = app.state::<App>();
    let _control = state.control.lock().unwrap();
    stop_active(&app)
}
/// Close a recording's tab, ending its capture first if it is the live one.
#[tauri::command(async)]
fn close_recording(app: AppHandle, rec: u32) -> Result<(), String> {
    let state = app.state::<App>();
    let _control = state.control.lock().unwrap();
    let live = state.recordings.lock().unwrap().live().map(|(id, _)| id);
    if live == Some(rec) {
        stop_active(&app)?;
    }
    state.recordings.lock().unwrap().list.remove(&rec);
    Ok(())
}
#[tauri::command(async)]
fn get_view(
    app: AppHandle,
    rec: u32,
    ch: String,
    t0: f64,
    t1: f64,
    px: usize,
) -> Result<tauri::ipc::Response, String> {
    if !t0.is_finite() || !t1.is_finite() || t1 < t0 || px == 0 || px > 32768 {
        return Err("invalid view range".into());
    }
    let state = app.state::<App>();
    let guard = state.recordings.lock().unwrap();
    let rec = &guard.get(rec)?.rec;
    let view = rec
        .specs()
        .into_iter()
        .find(|s| s.name == ch)
        .and_then(|s| rec.view(s.id, t0, t1, px))
        .ok_or("channel is not recorded")?;
    drop(guard);
    let mut bytes = Vec::new();
    match view {
        View::Raw { t, v } => {
            bytes.push(0);
            bytes.extend_from_slice(&(t.len() as u32).to_le_bytes());
            for (time, value) in t.into_iter().zip(v) {
                bytes.extend_from_slice(&time.to_le_bytes());
                bytes.extend_from_slice(&value.to_le_bytes());
            }
        }
        View::Envelope { t, min, max } => {
            bytes.push(1);
            bytes.extend_from_slice(&(t.len() as u32).to_le_bytes());
            for ((time, lo), hi) in t.into_iter().zip(min).zip(max) {
                bytes.extend_from_slice(&time.to_le_bytes());
                bytes.extend_from_slice(&lo.to_le_bytes());
                bytes.extend_from_slice(&hi.to_le_bytes());
            }
        }
    }
    Ok(tauri::ipc::Response::new(bytes))
}
#[derive(Serialize)]
struct AllStats {
    channels: BTreeMap<String, rec::Stats>,
    charge_mah: Option<f64>,
    energy: Option<rec::Energy>,
}
#[tauri::command(async)]
fn get_stats(app: AppHandle, rec: u32, t0: f64, t1: f64) -> Result<AllStats, String> {
    if !t0.is_finite() || !t1.is_finite() || t1 <= t0 {
        return Err("invalid stats range".into());
    }
    let state = app.state::<App>();
    let guard = state.recordings.lock().unwrap();
    let rec = &guard.get(rec)?.rec;
    Ok(AllStats {
        channels: rec
            .specs()
            .into_iter()
            .filter_map(|s| rec.stats(s.id, t0, t1).map(|v| (s.name, v)))
            .collect(),
        charge_mah: rec.charge_mah(t0, t1),
        energy: rec.energy(t0, t1),
    })
}
#[tauri::command(async)]
fn get_uart(app: AppHandle, rec: u32, t0: f64, t1: f64) -> Vec<UartChunk> {
    let state = app.state::<App>();
    let guard = state.recordings.lock().unwrap();
    let Ok(Rec { rec, .. }) = guard.get(rec) else {
        return Vec::new();
    };
    let mut chunks: Vec<UartChunk> = rec
        .specs()
        .into_iter()
        .filter(|s| s.kind == Kind::Bytes)
        .flat_map(|s| {
            let dir = if s.set { "tx" } else { "rx" };
            rec.bytes(s.id, t0, t1).into_iter().map(move |c| UartChunk {
                t: c.t,
                bytes: c.bytes,
                dir,
            })
        })
        .collect();
    chunks.sort_by(|a, b| a.t.total_cmp(&b.t));
    chunks
}
#[tauri::command(async)]
fn get_edges(app: AppHandle, rec: u32, t0: f64, t1: f64) -> Vec<rec::Edge> {
    let state = app.state::<App>();
    let guard = state.recordings.lock().unwrap();
    guard.get(rec).map_or(Vec::new(), |r| r.rec.edges(t0, t1))
}
#[tauri::command(async)]
fn save_recording(app: AppHandle, rec: u32, path: String) -> Result<(), String> {
    let state = app.state::<App>();
    let guard = state.recordings.lock().unwrap();
    let r = guard.get(rec)?;
    r.rec.save(path, &r.meta).map_err(text)
}
/// Open a saved recording in a tab of its own.
#[tauri::command(async)]
fn load_recording(app: AppHandle, path: String) -> Result<RecordingInfo, String> {
    let (rec, meta) = Recording::load(path).map_err(text)?;
    let state = app.state::<App>();
    Ok(state.recordings.lock().unwrap().add(Rec {
        rec,
        capture: None,
        device: None,
        meta,
    }))
}
#[tauri::command(async)]
fn export_recording(
    app: AppHandle,
    rec: u32,
    path: String,
    t0: Option<f64>,
    t1: Option<f64>,
) -> Result<(), String> {
    let state = app.state::<App>();
    let guard = state.recordings.lock().unwrap();
    let rec = &guard.get(rec)?.rec;
    let range = match (t0, t1) {
        (Some(a), Some(b)) => Some((a, b)),
        (None, None) => None,
        _ => return Err("incomplete range".into()),
    };
    let extension = std::path::Path::new(&path)
        .extension()
        .and_then(|x| x.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match extension.as_str() {
        "csv" => export::export_csv(rec, path, range).map_err(text),
        "sr" => export::export_sr_range(rec, path, range).map_err(text),
        _ => Err("export format must be .csv or .sr".into()),
    }
}

/// Open the chosen device, show it and start its worker.
fn open_device(app: &AppHandle, found: &Found) -> Result<(), Error> {
    let dev = benchwatt_session::open(found)?;
    let events = dev.events();
    let info = dev.info();
    let mut desc = DeviceDesc {
        id: String::new(),
        kind: found.kind.into(),
        model: info.model,
        serial: info.serial,
        firmware: info.firmware,
        hardware: info.hardware,
        channels: dev.channels(),
        outputs: dev.supply().map_or(Vec::new(), |s| s.outputs()),
        settings: dev.settings(),
        gpo: dev.gpio().map_or(0, |g| g.outputs().len()),
        uart: dev.uart().is_some(),
    };
    let state = match read_state(dev.as_ref(), &desc.settings, false) {
        Ok((outputs, state)) => {
            desc.outputs = outputs;
            Some(state)
        }
        Err(e) if e.kind == ErrorKind::Closed => return Err(e),
        Err(_) => None,
    };
    let app_state = app.state::<App>();
    let open = {
        let mut devices = app_state.devices.lock().unwrap();
        // The choice may have changed while the device was being opened.
        if !app_state.running.load(Ordering::Acquire) || devices.wanted.as_ref() != Some(found) {
            return Ok(());
        }
        desc.id = key(found);
        let open = Arc::new(Open {
            found: found.clone(),
            id: desc.id.clone(),
            dev: Mutex::new(dev),
            listed: Mutex::new(Listed { desc, state }),
            uart_open: AtomicBool::new(false),
            gone: AtomicBool::new(false),
        });
        devices.open = Some(open.clone());
        devices.error = None;
        open
    };
    emit_devices(app);
    let handle = app.clone();
    let worker = thread::spawn(move || device_worker(handle, open, events));
    app_state.threads.lock().unwrap().push(worker);
    Ok(())
}
/// Keeps the list of plugged-in devices current and the chosen one open.
fn scanner(app: AppHandle, wake: Receiver<()>) {
    let state = app.state::<App>();
    while state.running.load(Ordering::Acquire) {
        let found = benchwatt_session::find();
        let (changed, close, open) = {
            let mut devices = state.devices.lock().unwrap();
            let changed = devices.found != found;
            devices.found = found;
            let close = devices
                .open
                .clone()
                .filter(|o| devices.wanted.as_ref() != Some(&o.found));
            let open = devices
                .wanted
                .clone()
                .filter(|w| devices.open.is_none() && devices.found.contains(w));
            (changed, close, open)
        };
        if changed {
            emit_devices(&app);
        }
        if let Some(old) = close {
            // Its worker takes it off the list; the port is free once nobody holds it.
            old.gone.store(true, Ordering::SeqCst);
            while Arc::strong_count(&old) > 1 && state.running.load(Ordering::Acquire) {
                thread::sleep(Duration::from_millis(5));
            }
            continue;
        }
        if let Some(found) = open
            && let Err(e) = open_device(&app, &found)
        {
            let message = Some(e.to_string());
            let new = {
                let mut devices = state.devices.lock().unwrap();
                let new = devices.error != message && devices.wanted.as_ref() == Some(&found);
                if new {
                    devices.error = message;
                }
                new
            };
            if new {
                emit_devices(&app);
            }
        }
        let _ = wake.recv_timeout(Duration::from_secs(1));
    }
}
fn device_worker(app: AppHandle, open: Arc<Open>, events: Receiver<Event>) {
    let state = app.state::<App>();
    let names: HashMap<u16, String> = open
        .listed
        .lock()
        .unwrap()
        .desc
        .channels
        .iter()
        .map(|c| (c.id, c.name.clone()))
        .collect();
    let mut monitor = Monitor {
        device: open.id.clone(),
        readings: BTreeMap::new(),
        inputs: None,
    };
    let mut dirty = false;
    let mut last_monitor = Instant::now();
    let mut last_recording = Instant::now();
    while state.running.load(Ordering::Acquire) && !open.gone.load(Ordering::SeqCst) {
        let mut reread = None;
        match events.recv_timeout(Duration::from_millis(25)) {
            Ok(Event::Disconnected) | Err(RecvTimeoutError::Disconnected) => break,
            Ok(Event::Reading { ch, value }) => {
                if let Some(name) = names.get(&ch) {
                    dirty |= monitor.readings.insert(name.clone(), value) != Some(value);
                }
            }
            Ok(Event::Inputs(mask)) => {
                dirty |= monitor.inputs.replace(mask) != Some(mask);
            }
            Ok(Event::Changed) => reread = Some(false),
            Ok(Event::Alert {
                id,
                name,
                title,
                detail,
                severity,
            }) => {
                let alert = Alert {
                    device: &open.id,
                    id,
                    name,
                    title,
                    detail,
                    severity,
                };
                let _ = app.emit("alert", alert);
                reread = Some(true);
            }
            Err(RecvTimeoutError::Timeout) => {}
        }
        if let Some(force) = reread {
            let dev = open.dev.lock().unwrap();
            if let Err(e) = refresh(&app, &open, dev.as_ref(), force)
                && e.kind == ErrorKind::Closed
            {
                break;
            }
        }
        let info = {
            let mut recs = state.recordings.lock().unwrap();
            match recs.live() {
                Some((id, r)) if r.device.as_deref() == Some(&open.id) => {
                    if let Some(capture) = r.capture.as_mut() {
                        capture.pump(&mut r.rec, Duration::ZERO);
                    }
                    let due = last_recording.elapsed() >= Duration::from_millis(250);
                    due.then(|| {
                        last_recording = Instant::now();
                        rec_info(id, r)
                    })
                }
                _ => None,
            }
        };
        if let Some(info) = info {
            let _ = app.emit("recording", info);
        }
        // A linked serial port that was unplugged closes itself; the recording goes on.
        let unplugged = {
            let mut linked = state.serial.lock().unwrap();
            match linked.link.as_ref().is_some_and(SerialLink::closed) {
                true => {
                    linked.link = None;
                    linked.port.clone()
                }
                false => None,
            }
        };
        if let Some(port) = unplugged {
            emit_serial(&app);
            let alert = Alert {
                device: &open.id,
                id: 0,
                name: "serial".into(),
                title: "Serial port closed".into(),
                detail: format!("{port} went away."),
                severity: Severity::Warning,
            };
            let _ = app.emit("alert", alert);
        }
        if dirty && last_monitor.elapsed() >= Duration::from_millis(100) {
            let _ = app.emit("monitor", monitor.clone());
            last_monitor = Instant::now();
            dirty = false;
        }
    }
    if state.running.load(Ordering::Acquire) {
        remove(&app, &open);
    }
}
/// Forget a device that went away, ending a recording that was running on it.
fn remove(app: &AppHandle, open: &Open) {
    let state = app.state::<App>();
    open.gone.store(true, Ordering::SeqCst);
    let ended: Vec<RecordingInfo> = {
        let mut recs = state.recordings.lock().unwrap();
        recs.list
            .iter_mut()
            .filter(|(_, r)| r.device.as_deref() == Some(&open.id))
            .map(|(&id, r)| {
                if let Some(mut capture) = r.capture.take() {
                    capture.pump(&mut r.rec, Duration::ZERO);
                    r.rec.stop();
                }
                r.device = None;
                rec_info(id, r)
            })
            .collect()
    };
    for info in ended {
        let _ = app.emit("recording", info);
    }
    // The serial port was linked to this device; it goes with it.
    if state.serial.lock().unwrap().link.take().is_some() {
        emit_serial(app);
    }
    {
        let mut devices = state.devices.lock().unwrap();
        if devices
            .open
            .as_ref()
            .is_some_and(|o| std::ptr::eq(o.as_ref(), open))
        {
            devices.open = None;
        }
    }
    emit_devices(app);
}

fn main() {
    let (wake, woken) = mpsc::channel();
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(App {
            control: Mutex::new(()),
            recordings: Mutex::new(Recs::default()),
            devices: Mutex::new(Devices::default()),
            serial: Mutex::new(Linked::default()),
            threads: Mutex::new(Vec::new()),
            running: AtomicBool::new(true),
            wake: Mutex::new(wake),
        })
        .invoke_handler(tauri::generate_handler![
            get_devices,
            select_device,
            set_output,
            set_voltage,
            set_current_limit,
            set_setting,
            set_gpo,
            serial_ports,
            serial_info,
            serial_open,
            serial_close,
            serial_send,
            serial_lines,
            uart_open,
            uart_close,
            uart_send,
            start_recording,
            stop_recording,
            close_recording,
            recordings,
            get_view,
            get_stats,
            get_edges,
            get_uart,
            save_recording,
            load_recording,
            export_recording
        ])
        .setup(move |app| {
            let handle = app.handle().clone();
            let scan = thread::spawn(move || scanner(handle, woken));
            app.state::<App>().threads.lock().unwrap().push(scan);
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("Tauri application failed")
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event {
                let state = app.state::<App>();
                state.running.store(false, Ordering::Release);
                let _ = state.wake.lock().unwrap().send(());
                loop {
                    let Some(thread) = state.threads.lock().unwrap().pop() else {
                        break;
                    };
                    let _ = thread.join();
                }
                {
                    let _control = state.control.lock().unwrap();
                    let _ = stop_active(app);
                }
                state.serial.lock().unwrap().link = None;
                state.devices.lock().unwrap().open = None;
            }
        });
}
