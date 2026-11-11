use benchwatt_session::{
    Capture, Found,
    device::{
        ChannelDesc, Device, Error, ErrorKind, Event, Kind, OutputDesc, OutputState, SettingDesc,
        SettingKind, Severity, Supply, Value,
    },
    rec::{self, Recording, View, export, file},
};
use serde::Serialize;
use std::{
    collections::{BTreeMap, HashMap},
    fmt::Display,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{Receiver, RecvTimeoutError},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use tauri::{AppHandle, Emitter, Manager};

/// Lock order: control → Open::dev → recording → devices → Open::listed → threads.
/// Only device calls may block while one is held.
struct App {
    /// Serialises starting, stopping and loading recordings.
    control: Mutex<()>,
    recording: Mutex<Rec>,
    devices: Mutex<Vec<Arc<Open>>>,
    threads: Mutex<Vec<JoinHandle<()>>>,
    running: AtomicBool,
}
struct Open {
    found: Found,
    id: String,
    dev: Mutex<Box<dyn Device>>,
    listed: Mutex<Listed>,
    uart_open: AtomicBool,
    gone: AtomicBool,
}
#[derive(Default)]
struct Rec {
    rec: Option<Recording>,
    capture: Option<Capture>,
    /// The device the recording came from, while it is open.
    device: Option<String>,
    meta: file::Meta,
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
    active: bool,
    duration_s: f64,
    samples: HashMap<String, usize>,
    gaps: usize,
    channels: Vec<ChannelDesc>,
    device: Option<String>,
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
fn rec_info(r: &Rec) -> Option<RecordingInfo> {
    r.rec.as_ref().map(|rec| RecordingInfo {
        active: rec.active(),
        duration_s: rec.duration(),
        samples: rec.sample_counts(),
        gaps: rec.gaps().len(),
        channels: rec.specs(),
        device: r.device.clone(),
    })
}
fn listing(app: &AppHandle) -> Vec<Listed> {
    let state = app.state::<App>();
    let devices = state.devices.lock().unwrap();
    devices
        .iter()
        .map(|o| o.listed.lock().unwrap().clone())
        .collect()
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
fn get_devices(app: AppHandle) -> Vec<Listed> {
    listing(&app)
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

#[tauri::command(async)]
fn uart_open(app: AppHandle, device: String, baud: u32) -> Result<(), String> {
    if baud == 0 || baud > 3_000_000 {
        return Err("invalid baud rate".into());
    }
    let open = find_open(&app, &device)?;
    let dev = open.dev.lock().unwrap();
    let uart = dev.uart().ok_or("device has no UART")?;
    uart.open(baud).map_err(text)?;
    {
        let state = app.state::<App>();
        let mut r = state.recording.lock().unwrap();
        let r = &mut *r;
        if r.device.as_deref() == Some(&open.id)
            && r.capture.is_some()
            && let Some(rec) = r.rec.as_mut()
            && !rec.specs().iter().any(|c| c.id == uart.rx())
            && let Some(rx) = dev.channels().into_iter().find(|c| c.id == uart.rx())
        {
            rec.add_channel(rx);
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
    let mut r = state.recording.lock().unwrap();
    let r = &mut *r;
    let rec = match r.device.as_deref() == Some(&open.id) && r.capture.is_some() {
        true => r.rec.as_mut(),
        false => None,
    };
    benchwatt_session::uart_write(dev.as_ref(), rec, text.as_bytes()).map_err(|e| e.to_string())
}

/// Stop the running capture, if any, keeping what it delivered. The caller holds `control`.
fn stop_active(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<App>();
    let id = {
        let r = state.recording.lock().unwrap();
        r.capture.as_ref().and(r.device.clone())
    };
    // A capture whose device is gone is ended by that device's worker.
    let Some(open) = id.and_then(|id| find_open(app, &id).ok()) else {
        return Ok(());
    };
    let dev = open.dev.lock().unwrap();
    let (result, info) = {
        let mut r = state.recording.lock().unwrap();
        let r = &mut *r;
        let (Some(capture), Some(rec)) = (r.capture.take(), r.rec.as_mut()) else {
            return Ok(());
        };
        (capture.stop(dev.as_ref(), rec), rec_info(r))
    };
    let _ = app.emit("recording", info);
    result.map_err(text)
}
#[tauri::command(async)]
fn recording_info(app: AppHandle) -> Option<RecordingInfo> {
    rec_info(&app.state::<App>().recording.lock().unwrap())
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
    let (capture, rec) = Capture::start(dev.as_ref(), &names, false, None).map_err(text)?;
    let meta = benchwatt_session::meta(dev.as_ref());
    let mut r = state.recording.lock().unwrap();
    if open.gone.load(Ordering::SeqCst) {
        return Err(format!("device {device} is not connected"));
    }
    *r = Rec {
        rec: Some(rec),
        capture: Some(capture),
        device: Some(device),
        meta,
    };
    rec_info(&r).ok_or_else(|| "no recording".into())
}
#[tauri::command(async)]
fn stop_recording(app: AppHandle) -> Result<Option<RecordingInfo>, String> {
    let state = app.state::<App>();
    let _control = state.control.lock().unwrap();
    stop_active(&app)?;
    Ok(rec_info(&state.recording.lock().unwrap()))
}
#[tauri::command(async)]
fn get_view(
    app: AppHandle,
    ch: String,
    t0: f64,
    t1: f64,
    px: usize,
) -> Result<tauri::ipc::Response, String> {
    if !t0.is_finite() || !t1.is_finite() || t1 < t0 || px == 0 || px > 32768 {
        return Err("invalid view range".into());
    }
    let state = app.state::<App>();
    let guard = state.recording.lock().unwrap();
    let rec = guard.rec.as_ref().ok_or("no recording")?;
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
fn get_stats(app: AppHandle, t0: f64, t1: f64) -> Result<AllStats, String> {
    if !t0.is_finite() || !t1.is_finite() || t1 <= t0 {
        return Err("invalid stats range".into());
    }
    let state = app.state::<App>();
    let guard = state.recording.lock().unwrap();
    let rec = guard.rec.as_ref().ok_or("no recording")?;
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
fn get_uart(app: AppHandle, t0: f64, t1: f64) -> Vec<UartChunk> {
    let state = app.state::<App>();
    let guard = state.recording.lock().unwrap();
    let Some(rec) = guard.rec.as_ref() else {
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
fn get_edges(app: AppHandle, t0: f64, t1: f64) -> Vec<rec::Edge> {
    let state = app.state::<App>();
    let guard = state.recording.lock().unwrap();
    guard.rec.as_ref().map_or(Vec::new(), |r| r.edges(t0, t1))
}
#[tauri::command(async)]
fn save_recording(app: AppHandle, path: String) -> Result<(), String> {
    let state = app.state::<App>();
    let guard = state.recording.lock().unwrap();
    let rec = guard.rec.as_ref().ok_or("no recording")?;
    rec.save(path, &guard.meta).map_err(text)
}
#[tauri::command(async)]
fn load_recording(app: AppHandle, path: String) -> Result<RecordingInfo, String> {
    let (loaded, meta) = Recording::load(path).map_err(text)?;
    let state = app.state::<App>();
    let _control = state.control.lock().unwrap();
    stop_active(&app)?;
    let mut r = state.recording.lock().unwrap();
    *r = Rec {
        rec: Some(loaded),
        capture: None,
        device: None,
        meta,
    };
    rec_info(&r).ok_or_else(|| "no recording".into())
}
#[tauri::command(async)]
fn export_recording(
    app: AppHandle,
    path: String,
    t0: Option<f64>,
    t1: Option<f64>,
) -> Result<(), String> {
    let state = app.state::<App>();
    let guard = state.recording.lock().unwrap();
    let rec = guard.rec.as_ref().ok_or("no recording")?;
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

/// Open a found device, list it and start its worker.
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
        if !app_state.running.load(Ordering::Acquire) {
            return Ok(());
        }
        let kind = found.kind;
        desc.id = (1..)
            .map(|i| match i {
                1 => kind.to_string(),
                _ => format!("{kind}-{i}"),
            })
            .find(|id| !devices.iter().any(|o| &o.id == id))
            .unwrap_or_default();
        let open = Arc::new(Open {
            found: found.clone(),
            id: desc.id.clone(),
            dev: Mutex::new(dev),
            listed: Mutex::new(Listed { desc, state }),
            uart_open: AtomicBool::new(false),
            gone: AtomicBool::new(false),
        });
        devices.push(open.clone());
        open
    };
    emit_devices(app);
    let handle = app.clone();
    let worker = thread::spawn(move || device_worker(handle, open, events));
    app_state.threads.lock().unwrap().push(worker);
    Ok(())
}
fn scanner(app: AppHandle) {
    let state = app.state::<App>();
    let mut errors: HashMap<(&str, String), String> = HashMap::new();
    while state.running.load(Ordering::Acquire) {
        for found in benchwatt_session::find() {
            if !state.running.load(Ordering::Acquire) {
                break;
            }
            if state
                .devices
                .lock()
                .unwrap()
                .iter()
                .any(|o| o.found == found)
            {
                continue;
            }
            let key = (found.kind, found.port.clone());
            match open_device(&app, &found) {
                Ok(()) => {
                    errors.remove(&key);
                }
                Err(e) => {
                    let message = e.to_string();
                    if errors.get(&key) != Some(&message) {
                        eprintln!("open {} {}: {message}", found.kind, found.port);
                        errors.insert(key, message);
                    }
                }
            }
        }
        for _ in 0..20 {
            if !state.running.load(Ordering::Acquire) {
                break;
            }
            thread::sleep(Duration::from_millis(50));
        }
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
    while state.running.load(Ordering::Acquire) {
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
            let mut r = state.recording.lock().unwrap();
            let r = &mut *r;
            if r.device.as_deref() == Some(&open.id)
                && let (Some(capture), Some(rec)) = (r.capture.as_ref(), r.rec.as_mut())
            {
                capture.pump(rec, Duration::ZERO);
                if last_recording.elapsed() >= Duration::from_millis(250) {
                    last_recording = Instant::now();
                    rec_info(r)
                } else {
                    None
                }
            } else {
                None
            }
        };
        if let Some(info) = info {
            let _ = app.emit("recording", info);
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
    let info = {
        let mut r = state.recording.lock().unwrap();
        let r = &mut *r;
        if r.device.as_deref() == Some(&open.id) {
            if let (Some(capture), Some(rec)) = (r.capture.take(), r.rec.as_mut()) {
                capture.pump(rec, Duration::ZERO);
            }
            if let Some(rec) = r.rec.as_mut() {
                rec.stop();
            }
            r.device = None;
            rec_info(r)
        } else {
            None
        }
    };
    if let Some(info) = info {
        let _ = app.emit("recording", info);
    }
    state
        .devices
        .lock()
        .unwrap()
        .retain(|o| !std::ptr::eq(o.as_ref(), open));
    emit_devices(app);
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(App {
            control: Mutex::new(()),
            recording: Mutex::new(Rec::default()),
            devices: Mutex::new(Vec::new()),
            threads: Mutex::new(Vec::new()),
            running: AtomicBool::new(true),
        })
        .invoke_handler(tauri::generate_handler![
            get_devices,
            set_output,
            set_voltage,
            set_current_limit,
            set_setting,
            set_gpo,
            uart_open,
            uart_close,
            uart_send,
            start_recording,
            stop_recording,
            recording_info,
            get_view,
            get_stats,
            get_edges,
            get_uart,
            save_recording,
            load_recording,
            export_recording
        ])
        .setup(|app| {
            let handle = app.handle().clone();
            let scan = thread::spawn(move || scanner(handle));
            app.state::<App>().threads.lock().unwrap().push(scan);
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("Tauri application failed")
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event {
                let state = app.state::<App>();
                state.running.store(false, Ordering::Release);
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
                state.devices.lock().unwrap().clear();
            }
        });
}
