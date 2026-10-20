use otii_arc::prop::{CHANNELS, channel_by_name};
use otii_arc::{Arc as OtiiArc, Error, Event, PropertyValue, device::find_port};
use otii_rec::{ChannelSpec, Kind, Recording, View};
use serde::Serialize;
use std::{
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
use tauri::{Emitter, Manager};

struct DeviceState {
    arc: Mutex<Option<OtiiArc>>,
    recording: Mutex<Option<Recording>>,
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
    gpo: [Option<bool>; 3],
    fourwire: Option<bool>,
}
#[derive(Clone, Serialize)]
struct DeviceInfo {
    name: Option<String>,
    fw: Option<String>,
    hw: Option<String>,
    uid: Option<String>,
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
    name: &'static str,
}

fn bool_value(value: PropertyValue) -> Option<bool> {
    if let PropertyValue::Bool(v) = value {
        Some(v)
    } else {
        None
    }
}
fn number_value(value: PropertyValue) -> Option<f64> {
    if let PropertyValue::Number(v) = value {
        Some(v)
    } else {
        None
    }
}
fn int_value(value: PropertyValue) -> Option<i32> {
    if let PropertyValue::Enum(v) | PropertyValue::Int(v) = value {
        Some(v)
    } else {
        None
    }
}
fn optional<T>(
    arc: &OtiiArc,
    id: u32,
    decode: impl FnOnce(PropertyValue) -> Option<T>,
) -> Result<Option<T>, Error> {
    match arc.get_value(id) {
        Ok(value) => Ok(decode(value)),
        Err(Error::Status(-3)) => Ok(None),
        Err(error) => Err(error),
    }
}
fn read_settings(arc: &OtiiArc) -> Result<Settings, Error> {
    let gpio = optional(arc, 50, int_value)?;
    Ok(Settings {
        main: optional(arc, 9, bool_value)?,
        mainvolt: optional(arc, 11, number_value)?,
        maxcur: optional(arc, 12, number_value)?,
        mainsrccurrmode: optional(arc, 6, int_value)?,
        mainvolt_max: optional(arc, 0x80b, number_value)?,
        expvolt: optional(arc, 51, number_value)?,
        gpo: std::array::from_fn(|n| gpio.map(|v| ((v >> (n * 3)) & 7) == 2)),
        fourwire: optional(arc, 5, bool_value)?,
    })
}
fn info(arc: &OtiiArc) -> DeviceInfo {
    let props = arc.info().properties;
    let string = |id| match props.get(&id) {
        Some(PropertyValue::String(s)) => Some(s.clone()),
        _ => None,
    };
    DeviceInfo {
        name: string(1),
        hw: string(0x80),
        fw: string(0x81),
        uid: string(0x82),
    }
}
fn snapshot(state: &DeviceState) -> Result<StateSnapshot, Error> {
    let guard = state.arc.lock().unwrap();
    if let Some(arc) = guard.as_ref() {
        Ok(StateSnapshot {
            connection: Connection {
                connected: true,
                info: Some(info(arc)),
            },
            settings: Some(read_settings(arc)?),
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
    op: impl FnOnce(&OtiiArc) -> Result<(), Error>,
) -> Result<Settings, Error> {
    let guard = state.arc.lock().unwrap();
    let arc = guard.as_ref().ok_or(Error::Closed)?;
    op(arc)?;
    read_settings(arc)
}
#[tauri::command]
fn set_output(state: tauri::State<'_, DeviceState>, on: bool) -> Result<Settings, Error> {
    set_and_read(state, |arc| arc.output(on))
}
#[tauri::command]
fn set_voltage(state: tauri::State<'_, DeviceState>, volts: f64) -> Result<Settings, Error> {
    set_and_read(state, |arc| {
        let max = optional(arc, 0x80b, number_value)?;
        if !volts.is_finite() || volts < 0.0 || max.is_some_and(|v| volts > v) {
            return Err(Error::Property(format!(
                "voltage must be between 0 and {} V",
                max.map_or("device maximum".into(), |v| v.to_string())
            )));
        }
        arc.set_voltage(volts)
    })
}
#[tauri::command]
fn set_current_limit(state: tauri::State<'_, DeviceState>, amps: f64) -> Result<Settings, Error> {
    if !amps.is_finite() || !(0.001..=3.0).contains(&amps) {
        return Err(Error::Property(
            "current limit must be between 0.001 and 3.0 A".into(),
        ));
    }
    set_and_read(state, |arc| arc.set_current_limit(amps))
}
#[tauri::command]
fn set_current_mode(state: tauri::State<'_, DeviceState>, mode: String) -> Result<Settings, Error> {
    let value = match mode.as_str() {
        "cutoff" => 0,
        "limit" => 1,
        _ => return Err(Error::Property("mode must be cutoff or limit".into())),
    };
    set_and_read(state, |arc| arc.set(6, &PropertyValue::Enum(value)))
}
#[tauri::command]
fn set_gpo(state: tauri::State<'_, DeviceState>, n: u8, on: bool) -> Result<Settings, Error> {
    set_and_read(state, |arc| arc.set_gpo(n, on))
}

#[derive(Clone, Serialize)]
struct RecordingInfo {
    active: bool,
    duration_s: f64,
    samples: std::collections::HashMap<String, usize>,
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
#[tauri::command]
fn recording_info(state: tauri::State<'_, DeviceState>) -> Option<RecordingInfo> {
    state.recording.lock().unwrap().as_ref().map(rec_info)
}
#[tauri::command]
fn start_recording(
    state: tauri::State<'_, DeviceState>,
    channels: Vec<String>,
) -> Result<RecordingInfo, String> {
    let guard = state.arc.lock().unwrap();
    let arc = guard.as_ref().ok_or("device disconnected")?;
    let mut rec_guard = state.recording.lock().unwrap();
    let mut ids = Vec::new();
    for name in &channels {
        let id = channel_by_name(name).ok_or_else(|| format!("unknown channel: {name}"))?;
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    if state.uart_open.load(Ordering::Acquire) && !ids.contains(&0x17) {
        ids.push(0x17);
    }
    if ids.is_empty() {
        return Err("select at least one channel".into());
    }
    let info = arc.info();
    let specs: Vec<_> = ids
        .iter()
        .map(|&id| {
            let (_, name, unit) = CHANNELS.iter().find(|x| x.0 == id).unwrap();
            ChannelSpec {
                id,
                name: (*name).into(),
                unit: (*unit).into(),
                rate_hz: info.chinfo.get(id).rate_hz,
                kind: match id {
                    0x16 => Kind::Digital,
                    0x17 => Kind::Bytes,
                    _ => Kind::Analog,
                },
            }
        })
        .collect();
    if rec_guard.as_ref().is_some_and(Recording::active) {
        arc.stop_recording().map_err(|e| e.to_string())?;
        if let Some(rec) = rec_guard.as_mut() {
            rec.stop();
        }
    }
    arc.start_recording(&ids.iter().map(|&x| x as u32).collect::<Vec<_>>())
        .map_err(|e| e.to_string())?;
    let rec = Recording::new(&specs);
    let result = rec_info(&rec);
    *rec_guard = Some(rec);
    Ok(result)
}
#[tauri::command]
fn stop_recording(state: tauri::State<'_, DeviceState>) -> Result<Option<RecordingInfo>, String> {
    let guard = state.arc.lock().unwrap();
    let arc = guard.as_ref().ok_or("device disconnected")?;
    arc.stop_recording().map_err(|e| e.to_string())?;
    let mut rec = state.recording.lock().unwrap();
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
    let id = channel_by_name(&ch).ok_or("unknown channel")?;
    let guard = state.recording.lock().unwrap();
    let rec = guard.as_ref().ok_or("no recording")?;
    let view = rec.view(id, t0, t1, px).ok_or("channel is not recorded")?;
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
    channels: std::collections::HashMap<String, otii_rec::Stats>,
    charge_mah: Option<f64>,
    energy: Option<otii_rec::Energy>,
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
fn get_uart(state: tauri::State<'_, DeviceState>, t0: f64, t1: f64) -> Vec<otii_rec::UartChunk> {
    state
        .recording
        .lock()
        .unwrap()
        .as_ref()
        .map_or(Vec::new(), |r| r.uart(t0, t1))
}
#[tauri::command]
fn get_edges(state: tauri::State<'_, DeviceState>, t0: f64, t1: f64) -> Vec<otii_rec::Edge> {
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
    let guard = state.arc.lock().unwrap();
    let arc = guard.as_ref().ok_or("device disconnected")?;
    arc.uart_open(baud).map_err(|e| e.to_string())?;
    if let Some(rec) = state.recording.lock().unwrap().as_mut()
        && rec.active()
    {
        rec.add_channel(ChannelSpec {
            id: 0x17,
            name: "rx".into(),
            unit: "".into(),
            rate_hz: 0,
            kind: Kind::Bytes,
        });
    }
    state.uart_open.store(true, Ordering::Release);
    Ok(())
}
#[tauri::command]
fn uart_close(state: tauri::State<'_, DeviceState>) -> Result<(), String> {
    state
        .arc
        .lock()
        .unwrap()
        .as_ref()
        .ok_or("device disconnected")?
        .uart_close()
        .map_err(|e| e.to_string())?;
    state.uart_open.store(false, Ordering::Release);
    Ok(())
}
#[tauri::command]
fn uart_send(state: tauri::State<'_, DeviceState>, text: String) -> Result<(), String> {
    state
        .arc
        .lock()
        .unwrap()
        .as_ref()
        .ok_or("device disconnected")?
        .uart_write(text.as_bytes())
        .map_err(|e| e.to_string())
}

fn worker(app: tauri::AppHandle) {
    let state = app.state::<DeviceState>();
    while state.running.load(Ordering::Acquire) {
        let opened = find_port().map(|port| (OtiiArc::open(&port), port));
        if let Some((Err(e), port)) = &opened {
            eprintln!("open {port}: {e}");
        }
        if let Some((Ok(arc), _)) = opened {
            let events = arc.events();
            let initial = match read_settings(&arc) {
                Ok(settings) => Some(settings),
                Err(Error::Closed) => continue,
                Err(_) => None,
            };
            let device_info = info(&arc);
            {
                let mut guard = state.arc.lock().unwrap();
                if !state.running.load(Ordering::Acquire) {
                    break;
                }
                *guard = Some(arc);
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
                    Ok(Event::Disconnected)
                    | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                    Ok(Event::Monitor { ch, value }) => {
                        match ch {
                            0 => monitor.mc = Some(value),
                            1 => monitor.mv = Some(value),
                            6 => monitor.mp = Some(value),
                            0x10 => monitor.vb = Some(value),
                            0x11 => monitor.vj = Some(value),
                            0x14 => monitor.tp = Some(value),
                            0x16 => monitor.gpi = Some(value as u32),
                            _ => {}
                        }
                        dirty = true;
                    }
                    Ok(Event::Gpi(mask)) => {
                        monitor.gpi = Some(mask);
                        dirty = true;
                    }
                    Ok(Event::FourWire(value)) => {
                        monitor.fourwire = Some(value != 0);
                        dirty = true;
                    }
                    Ok(Event::Property { .. }) => {
                        if let Some(arc) = state.arc.lock().unwrap().as_ref()
                            && let Ok(next) = read_settings(arc)
                            && settings.as_ref() != Some(&next)
                        {
                            let _ = app.emit("settings", next.clone());
                            settings = Some(next);
                        }
                    }
                    Ok(Event::Alert { id, name }) => {
                        let _ = app.emit("alert", Alert { id, name });
                        if let Some(arc) = state.arc.lock().unwrap().as_ref()
                            && let Ok(next) = read_settings(arc)
                        {
                            let _ = app.emit("settings", next.clone());
                            settings = Some(next);
                        }
                    }
                    Ok(Event::Data { ms, block }) => {
                        if let Some(rec) = state.recording.lock().unwrap().as_mut() {
                            rec.push_block(ms, &block);
                        }
                    }
                    Ok(_) | Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
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
            if let Some(rec) = state.recording.lock().unwrap().as_mut() {
                rec.stop();
                let _ = app.emit("recording", rec_info(rec));
            }
            state.uart_open.store(false, Ordering::Release);
            *state.arc.lock().unwrap() = None;
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
        .manage(DeviceState {
            arc: Mutex::new(None),
            recording: Mutex::new(None),
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
            start_recording,
            stop_recording,
            recording_info,
            get_view,
            get_stats,
            get_uart,
            get_edges,
            uart_open,
            uart_close,
            uart_send
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
                state.arc.lock().unwrap().take();
            }
        });
}
