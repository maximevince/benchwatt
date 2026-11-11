use benchwatt_session::{
    Capture,
    device::{Device, Error, ErrorKind, Event, Supply, Value},
    rec::{self, Recording, View, export, file},
};
use serde::Serialize;
use std::{
    collections::HashMap,
    fmt::Display,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::RecvTimeoutError,
    },
    thread,
    time::{Duration, Instant},
};
use tauri::{Emitter, Manager};

/// Lock order: dev → capture → recording → recording_device.
struct DeviceState {
    dev: Mutex<Option<Box<dyn Device>>>,
    capture: Mutex<Option<Capture>>,
    recording: Mutex<Option<Recording>>,
    recording_device: Mutex<file::DeviceInfo>,
    uart_open: AtomicBool,
    running: AtomicBool,
}

#[derive(Clone, Default, PartialEq, Serialize)]
struct Settings {
    main: Option<bool>,
    mainvolt: Option<f64>,
    maxcur: Option<f64>,
    mainsrccurrmode: Option<i32>,
    mainvolt_max: Option<f64>,
    expvolt: Option<f64>,
    five_volt: Option<bool>,
    gpo: [Option<bool>; 3],
    fourwire: Option<bool>,
}
#[derive(Clone, Serialize)]
struct DeviceInfo {
    name: Option<String>,
    fw: Option<String>,
    hw: Option<String>,
    uid: Option<String>,
    proto: u32,
}
#[derive(Clone, Serialize)]
struct Connection {
    connected: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    info: Option<DeviceInfo>,
}
#[derive(Serialize)]
struct StateSnapshot {
    connection: Connection,
    settings: Option<Settings>,
}
#[derive(Clone, Default, Serialize)]
struct Monitor {
    mv: Option<f32>,
    mc: Option<f32>,
    mp: Option<f32>,
    vb: Option<f32>,
    vj: Option<f32>,
    tp: Option<f32>,
    gpi: Option<u32>,
    fourwire: Option<bool>,
}
#[derive(Clone, Serialize)]
struct Alert {
    id: u16,
    name: String,
}

fn closed() -> Error {
    Error::new(ErrorKind::Closed, "device connection closed")
}
fn property(message: impl Display) -> Error {
    Error::invalid(format!("property error: {message}"))
}
fn supply(dev: &dyn Device) -> Result<&dyn Supply, Error> {
    dev.supply()
        .ok_or_else(|| Error::invalid("device has no supply"))
}
/// A key the device does not have reads as `None`; a lost connection is an error.
fn optional(dev: &dyn Device, key: &str) -> Result<Option<Value>, Error> {
    match dev.get(key) {
        Ok(value) => Ok(Some(value)),
        Err(e) if matches!(e.kind, ErrorKind::Closed | ErrorKind::Timeout) => Err(e),
        Err(_) => Ok(None),
    }
}
fn read_settings(dev: &dyn Device) -> Result<Settings, Error> {
    let (out, max) = match dev.supply() {
        Some(s) => (
            Some(s.state(1)?),
            s.outputs().first().map(|o| o.voltage.max),
        ),
        None => (None, None),
    };
    let mut gpo = [None; 3];
    if let Some(g) = dev.gpio() {
        for (slot, level) in gpo.iter_mut().zip(g.outputs()) {
            *slot = level;
        }
    }
    Ok(Settings {
        main: out.map(|o| o.enabled),
        mainvolt: out.map(|o| o.voltage),
        maxcur: out.map(|o| o.current_limit),
        mainsrccurrmode: optional(dev, "mainsrccurrmode")?
            .and_then(|v| v.as_i64())
            .map(|v| v as i32),
        mainvolt_max: max,
        expvolt: optional(dev, "expvolt")?.and_then(|v| v.as_f64()),
        five_volt: optional(dev, "expoutvolt")?
            .and_then(|v| v.as_f64())
            .map(|v| v > 0.0),
        gpo,
        fourwire: optional(dev, "4wire")?.and_then(|v| v.as_bool()),
    })
}
fn info(dev: &dyn Device) -> DeviceInfo {
    let d = benchwatt_session::meta(dev).device;
    DeviceInfo {
        name: d.name,
        fw: d.fw,
        hw: d.hw,
        uid: d.uid,
        proto: d.proto.unwrap_or(0),
    }
}
fn snapshot(state: &DeviceState) -> Result<StateSnapshot, Error> {
    let guard = state.dev.lock().unwrap();
    if let Some(dev) = guard.as_deref() {
        Ok(StateSnapshot {
            connection: Connection {
                connected: true,
                info: Some(info(dev)),
            },
            settings: Some(read_settings(dev)?),
        })
    } else {
        Ok(StateSnapshot {
            connection: Connection {
                connected: false,
                info: None,
            },
            settings: None,
        })
    }
}
#[tauri::command]
fn get_state(state: tauri::State<'_, DeviceState>) -> Result<StateSnapshot, Error> {
    snapshot(&state)
}
fn set_and_read(
    state: tauri::State<'_, DeviceState>,
    op: impl FnOnce(&dyn Device) -> Result<(), Error>,
) -> Result<Settings, Error> {
    let guard = state.dev.lock().unwrap();
    let dev = guard.as_deref().ok_or_else(closed)?;
    op(dev)?;
    read_settings(dev)
}
#[tauri::command]
fn set_output(state: tauri::State<'_, DeviceState>, on: bool) -> Result<Settings, Error> {
    set_and_read(state, |dev| supply(dev)?.set_enabled(1, on))
}
#[tauri::command]
fn set_voltage(state: tauri::State<'_, DeviceState>, volts: f64) -> Result<Settings, Error> {
    set_and_read(state, |dev| {
        let s = supply(dev)?;
        let range = s
            .outputs()
            .first()
            .map(|o| o.voltage)
            .ok_or_else(|| Error::invalid("device has no output"))?;
        if !range.contains(volts) {
            return Err(property(format_args!(
                "voltage must be between {} and {} V",
                range.min, range.max
            )));
        }
        s.set_voltage(1, volts)
    })
}
#[tauri::command]
fn set_current_limit(state: tauri::State<'_, DeviceState>, amps: f64) -> Result<Settings, Error> {
    if !amps.is_finite() || !(0.001..=3.0).contains(&amps) {
        return Err(property("current limit must be between 0.001 and 3.0 A"));
    }
    set_and_read(state, |dev| supply(dev)?.set_current_limit(1, amps))
}
#[tauri::command]
fn set_current_mode(state: tauri::State<'_, DeviceState>, mode: String) -> Result<Settings, Error> {
    let value = match mode.as_str() {
        "cutoff" => 0,
        "limit" => 1,
        _ => return Err(property("mode must be cutoff or limit")),
    };
    set_and_read(state, |dev| dev.set("mainsrccurrmode", value.into()))
}
#[tauri::command]
fn set_exp_voltage(state: tauri::State<'_, DeviceState>, volts: f64) -> Result<Settings, Error> {
    if !volts.is_finite() || !(1.2..=5.0).contains(&volts) {
        return Err(property("digital voltage must be between 1.2 and 5 V"));
    }
    set_and_read(state, |dev| dev.set("expvolt", volts.into()))
}
#[tauri::command]
fn set_5v(state: tauri::State<'_, DeviceState>, on: bool) -> Result<Settings, Error> {
    set_and_read(state, |dev| {
        dev.set("expoutvolt", (if on { 5.0 } else { 0.0 }).into())
    })
}
#[tauri::command]
fn set_gpo(state: tauri::State<'_, DeviceState>, n: u8, on: bool) -> Result<Settings, Error> {
    // The Arc has GPO1 and GPO2 (gpiocfg has room for a third).
    if !(1..=2).contains(&n) {
        return Err(property("GPO must be 1 or 2"));
    }
    set_and_read(state, |dev| {
        dev.gpio()
            .ok_or_else(|| Error::invalid("device has no GPIO"))?
            .set_output(n, on)
    })
}

#[derive(Clone, Serialize)]
struct RecordingInfo {
    active: bool,
    duration_s: f64,
    samples: HashMap<String, usize>,
    gaps: usize,
    channels: Vec<String>,
}
fn rec_info(rec: &Recording) -> RecordingInfo {
    RecordingInfo {
        active: rec.active(),
        duration_s: rec.duration(),
        samples: rec.sample_counts(),
        gaps: rec.gaps().len(),
        channels: rec.specs().into_iter().map(|s| s.name).collect(),
    }
}
/// Stop the running capture, if any, keeping what it delivered.
fn stop_capture(
    dev: &dyn Device,
    capture: &mut Option<Capture>,
    rec: &mut Option<Recording>,
) -> Result<(), Error> {
    let Some(c) = capture.take() else {
        return Ok(());
    };
    match rec {
        Some(rec) => c.stop(dev, rec),
        None => c.stop(dev, &mut Recording::new(&[])),
    }
}
#[tauri::command]
fn recording_info(state: tauri::State<'_, DeviceState>) -> Option<RecordingInfo> {
    state.recording.lock().unwrap().as_ref().map(rec_info)
}
#[tauri::command]
fn start_recording(
    state: tauri::State<'_, DeviceState>,
    channels: Vec<String>,
) -> Result<RecordingInfo, String> {
    let guard = state.dev.lock().unwrap();
    let dev = guard.as_deref().ok_or("device disconnected")?;
    let mut capture = state.capture.lock().unwrap();
    let mut rec_guard = state.recording.lock().unwrap();
    let all = dev.channels();
    let mut names: Vec<String> = Vec::new();
    for name in channels {
        if !all.iter().any(|c| c.name == name) {
            return Err(format!("unknown channel: {name}"));
        }
        if !names.contains(&name) {
            names.push(name);
        }
    }
    if state.uart_open.load(Ordering::Acquire)
        && let Some(rx) = dev.uart().and_then(|u| all.iter().find(|c| c.id == u.rx()))
        && !names.contains(&rx.name)
    {
        names.push(rx.name.clone());
    }
    if names.is_empty() {
        return Err("select at least one channel".into());
    }
    stop_capture(dev, &mut capture, &mut rec_guard).map_err(|e| e.to_string())?;
    let (started, rec) = Capture::start(dev, &names, false, None).map_err(|e| e.to_string())?;
    *state.recording_device.lock().unwrap() = benchwatt_session::meta(dev).device;
    let result = rec_info(&rec);
    *capture = Some(started);
    *rec_guard = Some(rec);
    Ok(result)
}
#[tauri::command]
fn stop_recording(state: tauri::State<'_, DeviceState>) -> Result<Option<RecordingInfo>, String> {
    let guard = state.dev.lock().unwrap();
    let dev = guard.as_deref().ok_or("device disconnected")?;
    let mut capture = state.capture.lock().unwrap();
    let mut rec = state.recording.lock().unwrap();
    stop_capture(dev, &mut capture, &mut rec).map_err(|e| e.to_string())?;
    if let Some(rec) = rec.as_mut() {
        rec.stop();
    }
    Ok(rec.as_ref().map(rec_info))
}
#[tauri::command]
fn get_view(
    state: tauri::State<'_, DeviceState>,
    ch: String,
    t0: f64,
    t1: f64,
    px: usize,
) -> Result<tauri::ipc::Response, String> {
    if !t0.is_finite() || !t1.is_finite() || t1 < t0 || px == 0 || px > 32768 {
        return Err("invalid view range".into());
    }
    let guard = state.recording.lock().unwrap();
    let rec = guard.as_ref().ok_or("no recording")?;
    let view = rec
        .specs()
        .into_iter()
        .find(|s| s.name == ch)
        .and_then(|s| rec.view(s.id, t0, t1, px))
        .ok_or("channel is not recorded")?;
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
    channels: std::collections::BTreeMap<String, rec::Stats>,
    charge_mah: Option<f64>,
    energy: Option<rec::Energy>,
}
#[tauri::command]
fn get_stats(state: tauri::State<'_, DeviceState>, t0: f64, t1: f64) -> Result<AllStats, String> {
    if !t0.is_finite() || !t1.is_finite() || t1 <= t0 {
        return Err("invalid stats range".into());
    }
    let guard = state.recording.lock().unwrap();
    let rec = guard.as_ref().ok_or("no recording")?;
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
#[tauri::command]
fn get_uart(
    state: tauri::State<'_, DeviceState>,
    ch: u16,
    t0: f64,
    t1: f64,
) -> Vec<rec::UartChunk> {
    state
        .recording
        .lock()
        .unwrap()
        .as_ref()
        .map_or(Vec::new(), |r| r.bytes(ch, t0, t1))
}
#[tauri::command]
fn get_edges(state: tauri::State<'_, DeviceState>, t0: f64, t1: f64) -> Vec<rec::Edge> {
    state
        .recording
        .lock()
        .unwrap()
        .as_ref()
        .map_or(Vec::new(), |r| r.edges(t0, t1))
}
#[tauri::command]
fn uart_open(state: tauri::State<'_, DeviceState>, baud: u32) -> Result<(), String> {
    if baud == 0 || baud > 3_000_000 {
        return Err("invalid baud rate".into());
    }
    let guard = state.dev.lock().unwrap();
    let dev = guard.as_deref().ok_or("device disconnected")?;
    let uart = dev.uart().ok_or("device has no UART")?;
    uart.open(baud).map_err(|e| e.to_string())?;
    if let Some(rec) = state.recording.lock().unwrap().as_mut()
        && rec.active()
        && let Some(rx) = dev.channels().into_iter().find(|c| c.id == uart.rx())
    {
        rec.add_channel(rx);
    }
    state.uart_open.store(true, Ordering::Release);
    Ok(())
}
#[tauri::command]
fn uart_close(state: tauri::State<'_, DeviceState>) -> Result<(), String> {
    state
        .dev
        .lock()
        .unwrap()
        .as_deref()
        .ok_or("device disconnected")?
        .uart()
        .ok_or("device has no UART")?
        .close()
        .map_err(|e| e.to_string())?;
    state.uart_open.store(false, Ordering::Release);
    Ok(())
}
#[tauri::command]
fn uart_send(state: tauri::State<'_, DeviceState>, text: String) -> Result<(), String> {
    let guard = state.dev.lock().unwrap();
    let dev = guard.as_deref().ok_or("device disconnected")?;
    let mut rec = state.recording.lock().unwrap();
    benchwatt_session::uart_write(dev, rec.as_mut(), text.as_bytes()).map_err(|e| e.to_string())
}
#[tauri::command]
fn save_recording(state: tauri::State<'_, DeviceState>, path: String) -> Result<(), String> {
    let rec = state.recording.lock().unwrap();
    let rec = rec.as_ref().ok_or("no recording")?;
    let meta = file::Meta {
        device: state.recording_device.lock().unwrap().clone(),
        ..file::Meta::default()
    };
    rec.save(path, &meta).map_err(|e| e.to_string())
}
#[tauri::command]
fn load_recording(
    state: tauri::State<'_, DeviceState>,
    path: String,
) -> Result<RecordingInfo, String> {
    let (loaded, meta) = Recording::load(path).map_err(|e| e.to_string())?;
    let guard = state.dev.lock().unwrap();
    let mut capture = state.capture.lock().unwrap();
    let mut rec = state.recording.lock().unwrap();
    if let Some(dev) = guard.as_deref() {
        stop_capture(dev, &mut capture, &mut rec).map_err(|e| e.to_string())?;
    }
    let result = rec_info(&loaded);
    *rec = Some(loaded);
    *state.recording_device.lock().unwrap() = meta.device;
    Ok(result)
}
#[tauri::command]
fn export_recording(
    state: tauri::State<'_, DeviceState>,
    path: String,
    t0: Option<f64>,
    t1: Option<f64>,
) -> Result<(), String> {
    let rec = state.recording.lock().unwrap();
    let rec = rec.as_ref().ok_or("no recording")?;
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
        "csv" => export::export_csv(rec, path, range).map_err(|e| e.to_string()),
        "sr" => export::export_sr_range(rec, path, range).map_err(|e| e.to_string()),
        _ => Err("export format must be .csv or .sr".into()),
    }
}

fn worker(app: tauri::AppHandle) {
    let state = app.state::<DeviceState>();
    while state.running.load(Ordering::Acquire) {
        let opened = benchwatt_session::find().map(|port| (benchwatt_session::open(&port), port));
        if let Some((Err(e), port)) = &opened {
            eprintln!("open {port}: {e}");
        }
        if let Some((Ok(dev), _)) = opened {
            let events = dev.events();
            let names: HashMap<u16, String> =
                dev.channels().into_iter().map(|c| (c.id, c.name)).collect();
            let initial = match read_settings(dev.as_ref()) {
                Ok(settings) => Some(settings),
                Err(e) if e.kind == ErrorKind::Closed => continue,
                Err(_) => None,
            };
            let device_info = info(dev.as_ref());
            {
                let mut guard = state.dev.lock().unwrap();
                if !state.running.load(Ordering::Acquire) {
                    break;
                }
                *guard = Some(dev);
            }
            let _ = app.emit(
                "connection",
                Connection {
                    connected: true,
                    info: Some(device_info),
                },
            );
            if let Some(settings) = initial.clone() {
                let _ = app.emit("settings", settings);
            }
            let mut settings = initial;
            let mut monitor = Monitor::default();
            let mut dirty = false;
            let mut last_emit = Instant::now();
            let mut last_recording_emit = Instant::now();
            while state.running.load(Ordering::Acquire) {
                match events.recv_timeout(Duration::from_millis(25)) {
                    Ok(Event::Disconnected) | Err(RecvTimeoutError::Disconnected) => break,
                    Ok(Event::Reading { ch, value }) => {
                        match names.get(&ch).map(String::as_str) {
                            Some("mc") => monitor.mc = Some(value),
                            Some("mv") => monitor.mv = Some(value),
                            Some("mp") => monitor.mp = Some(value),
                            Some("vb") => monitor.vb = Some(value),
                            Some("vj") => monitor.vj = Some(value),
                            Some("tp") => monitor.tp = Some(value),
                            Some("gpi") => monitor.gpi = Some(value as u32),
                            _ => {}
                        }
                        dirty = true;
                    }
                    Ok(Event::Inputs(mask)) => {
                        monitor.gpi = Some(mask);
                        dirty = true;
                    }
                    Ok(Event::Changed) => {
                        if let Some(dev) = state.dev.lock().unwrap().as_deref() {
                            if let Ok(next) = read_settings(dev)
                                && settings.as_ref() != Some(&next)
                            {
                                let _ = app.emit("settings", next.clone());
                                settings = Some(next);
                            }
                            if let Ok(Value::Bool(on)) = dev.get("4wire_active")
                                && monitor.fourwire != Some(on)
                            {
                                monitor.fourwire = Some(on);
                                dirty = true;
                            }
                        }
                    }
                    Ok(Event::Alert { id, name }) => {
                        let _ = app.emit("alert", Alert { id, name });
                        if let Some(dev) = state.dev.lock().unwrap().as_deref()
                            && let Ok(next) = read_settings(dev)
                        {
                            let _ = app.emit("settings", next.clone());
                            settings = Some(next);
                        }
                    }
                    Err(RecvTimeoutError::Timeout) => {}
                }
                if let Some(capture) = state.capture.lock().unwrap().as_ref()
                    && let Some(rec) = state.recording.lock().unwrap().as_mut()
                {
                    capture.pump(rec, Duration::ZERO);
                }
                if dirty && last_emit.elapsed() >= Duration::from_millis(100) {
                    let _ = app.emit("monitor", monitor.clone());
                    last_emit = Instant::now();
                    dirty = false;
                }
                if last_recording_emit.elapsed() >= Duration::from_millis(250) {
                    if let Some(rec) = state.recording.lock().unwrap().as_ref()
                        && rec.active()
                    {
                        let _ = app.emit("recording", rec_info(rec));
                    }
                    last_recording_emit = Instant::now();
                }
            }
            {
                let mut dev = state.dev.lock().unwrap();
                let mut capture = state.capture.lock().unwrap();
                let mut rec = state.recording.lock().unwrap();
                if let Some(capture) = capture.take()
                    && let Some(rec) = rec.as_mut()
                {
                    capture.pump(rec, Duration::ZERO);
                }
                if let Some(rec) = rec.as_mut() {
                    rec.stop();
                    let _ = app.emit("recording", rec_info(rec));
                }
                state.uart_open.store(false, Ordering::Release);
                *dev = None;
            }
            let _ = app.emit(
                "connection",
                Connection {
                    connected: false,
                    info: None,
                },
            );
        }
        thread::sleep(Duration::from_secs(1));
    }
}
fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(DeviceState {
            dev: Mutex::new(None),
            capture: Mutex::new(None),
            recording: Mutex::new(None),
            recording_device: Mutex::new(file::DeviceInfo::default()),
            uart_open: AtomicBool::new(false),
            running: AtomicBool::new(true),
        })
        .invoke_handler(tauri::generate_handler![
            get_state,
            set_output,
            set_voltage,
            set_current_limit,
            set_current_mode,
            set_gpo,
            set_exp_voltage,
            set_5v,
            start_recording,
            stop_recording,
            recording_info,
            get_view,
            get_stats,
            get_uart,
            get_edges,
            uart_open,
            uart_close,
            uart_send,
            save_recording,
            load_recording,
            export_recording
        ])
        .setup(|app| {
            let handle = app.handle().clone();
            thread::spawn(move || worker(handle));
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("Tauri application failed")
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event {
                let state = app.state::<DeviceState>();
                state.running.store(false, Ordering::Release);
                state.dev.lock().unwrap().take();
            }
        });
}
