//! Helpers for the offline tests: a recording replayed from captured Arc
//! traffic, and a pretend supply that behaves like a device without hardware.
use crate::{PyDevice, PyRecording, file_error};
use benchwatt_session::{
    device::{
        Block, ChannelDesc, Data, Device, DeviceInfo, Error, Event, Kind, Meter, OutputDesc,
        OutputState, Range, Result, SettingDesc, SettingKind, Severity, Supply, Value,
    },
    rec::{Recording, file::Meta},
};
use pyo3::prelude::*;
use serde_json::json;
use std::{
    collections::HashMap,
    sync::{
        Mutex,
        mpsc::{self, Receiver, Sender},
    },
};

#[pyfunction]
fn recording_from_fixture(path: &str) -> PyResult<PyRecording> {
    let input = std::fs::read_to_string(path).map_err(file_error)?;
    let (channels, blocks) = otii_arc::adapter::replay(&input);
    let keep: Vec<_> = channels
        .into_iter()
        .filter(|c| matches!(c.name.as_str(), "mc" | "mv" | "gpi" | "rx"))
        .collect();
    let mut rec = Recording::new(&keep);
    for b in &blocks {
        rec.push(b);
    }
    rec.stop();
    Ok(PyRecording {
        rec,
        meta: Meta::default(),
    })
}

/// A supply with `outputs` outputs. Channel `d` holds two digital inputs, and
/// `chN.v` and `chN.i` measure output N. The meter delivers 100 samples per
/// channel, then the stream ends as if the device were unplugged. Every event
/// subscriber first gets a reading, an input change and an alert.
struct Fake {
    outputs: u8,
    state: Mutex<Vec<OutputState>>,
    settings: Mutex<HashMap<String, Value>>,
}
impl Fake {
    fn new(outputs: u8) -> Self {
        let off = OutputState {
            enabled: false,
            voltage: None,
            current_limit: Some(1.0),
        };
        Self {
            outputs,
            state: Mutex::new(vec![off; outputs.into()]),
            settings: Mutex::new(HashMap::from([
                ("ovp1".into(), json!(5.0)),
                ("beep".into(), json!(true)),
            ])),
        }
    }
    fn update(&self, out: u8, f: impl FnOnce(&mut OutputState)) -> Result<()> {
        let mut state = self.state.lock().unwrap();
        let s = state
            .get_mut(usize::from(out).wrapping_sub(1))
            .ok_or_else(|| Error::invalid(format!("no output {out}")))?;
        f(s);
        Ok(())
    }
}
impl Device for Fake {
    fn info(&self) -> DeviceInfo {
        DeviceInfo {
            kind: "fake".into(),
            model: "Fake supply".into(),
            firmware: "1.0".into(),
            details: json!({"outputs": self.outputs})
                .as_object()
                .unwrap()
                .clone(),
            ..DeviceInfo::default()
        }
    }
    fn channels(&self) -> Vec<ChannelDesc> {
        let mut all = vec![ChannelDesc {
            bits: 2,
            ..ChannelDesc::new(0, "d", "Digital inputs", "", 1000, Kind::Digital)
        }];
        for n in 1..=u16::from(self.outputs) {
            for (id, q, label, unit) in [
                (2 * n - 1, "v", "Voltage", "V"),
                (2 * n, "i", "Current", "A"),
            ] {
                all.push(ChannelDesc {
                    group: format!("ch{n}"),
                    ..ChannelDesc::new(id, &format!("ch{n}.{q}"), label, unit, 1000, Kind::Analog)
                });
            }
        }
        all
    }
    fn settings(&self) -> Vec<SettingDesc> {
        vec![
            SettingDesc {
                key: "ovp1".into(),
                label: "Over-voltage 1".into(),
                kind: SettingKind::Number {
                    min: 0.0,
                    max: 33.0,
                    unit: "V".into(),
                },
                output: Some(1),
                readonly: false,
            },
            SettingDesc {
                key: "beep".into(),
                label: "Beep".into(),
                kind: SettingKind::Bool,
                output: None,
                readonly: false,
            },
        ]
    }
    fn get(&self, key: &str) -> Result<Value> {
        self.settings
            .lock()
            .unwrap()
            .get(key)
            .cloned()
            .ok_or_else(|| Error::invalid(format!("unknown setting {key}")))
    }
    fn set(&self, key: &str, value: Value) -> Result<()> {
        let mut settings = self.settings.lock().unwrap();
        let old = settings
            .get_mut(key)
            .ok_or_else(|| Error::invalid(format!("unknown setting {key}")))?;
        *old = value;
        Ok(())
    }
    fn events(&self) -> Receiver<Event> {
        let (tx, rx) = mpsc::channel();
        for e in [
            Event::Reading { ch: 1, value: 1.5 },
            Event::Inputs(2),
            Event::Alert {
                id: 7,
                name: "ovp".into(),
                title: "Over-voltage".into(),
                detail: "CH1 went over 5 V".into(),
                severity: Severity::Error,
            },
        ] {
            tx.send(e).unwrap();
        }
        rx
    }
    fn supply(&self) -> Option<&dyn Supply> {
        Some(self)
    }
    fn meter(&self) -> Option<&dyn Meter> {
        Some(self)
    }
}
impl Supply for Fake {
    fn outputs(&self) -> Vec<OutputDesc> {
        (1..=self.outputs)
            .map(|n| OutputDesc {
                n,
                label: format!("CH{n}"),
                group: format!("ch{n}"),
                voltage: Range {
                    min: 0.0,
                    max: 32.0,
                },
                voltage_steps: Vec::new(),
                current_limit: Some(Range { min: 0.0, max: 3.2 }),
                color: Some("yellow".into()),
            })
            .collect()
    }
    fn set_voltage(&self, out: u8, volts: f64) -> Result<()> {
        if !(0.0..=32.0).contains(&volts) {
            return Err(Error::invalid("voltage out of range"));
        }
        self.update(out, |s| s.voltage = Some(volts))
    }
    fn set_current_limit(&self, out: u8, amps: f64) -> Result<()> {
        self.update(out, |s| s.current_limit = Some(amps))
    }
    fn set_enabled(&self, out: u8, on: bool) -> Result<()> {
        self.update(out, |s| s.enabled = on)
    }
    fn state(&self, out: u8) -> Result<OutputState> {
        let mut state = None;
        self.update(out, |s| state = Some(*s))?;
        Ok(state.unwrap())
    }
}
impl Meter for Fake {
    fn start(&self, channels: &[u16], sink: Sender<Block>) -> Result<()> {
        for &ch in channels {
            let data = match ch {
                0 => Data::Digital((0..100).map(|i| (i / 25) % 2).collect()),
                _ => Data::Analog(vec![f32::from(ch) * 0.5; 100]),
            };
            let _ = sink.send(Block {
                ch,
                t: 0.0,
                dt: 0.001,
                data,
            });
        }
        Ok(())
    }
    fn stop(&self) -> Result<()> {
        Ok(())
    }
}

#[pyfunction]
#[pyo3(signature = (outputs=2))]
fn fake_device(outputs: u8) -> PyDevice {
    PyDevice::new(Box::new(Fake::new(outputs)))
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    let testing = PyModule::new(m.py(), "_testing")?;
    testing.add_function(wrap_pyfunction!(recording_from_fixture, &testing)?)?;
    testing.add_function(wrap_pyfunction!(fake_device, &testing)?)?;
    m.add_submodule(&testing)
}
