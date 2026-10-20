use otii_arc::{Arc as OtiiArc, Error, Event, PropertyValue, device::find_port};
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
                    Ok(_) | Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                }
                if dirty && last_emit.elapsed() >= Duration::from_millis(100) {
                    let _ = app.emit("monitor", monitor.clone());
                    last_emit = Instant::now();
                    dirty = false;
                }
            }
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
            running: AtomicBool::new(true),
        })
        .invoke_handler(tauri::generate_handler![
            get_state,
            set_output,
            set_voltage,
            set_current_limit,
            set_current_mode,
            set_gpo
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
