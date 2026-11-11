use benchwatt_session::{
    Capture,
    device::{Device, Error, ErrorKind, Event, Kind, OutputState, Supply, Value},
    rec::{Recording as NativeRecording, export, file::Meta},
};
use numpy::PyArray1;
use pyo3::{
    create_exception,
    exceptions::{PyException, PyTimeoutError, PyValueError},
    prelude::*,
    types::{PyBytes, PyDict},
};
use serde_json::json;
use std::{
    collections::HashMap,
    sync::{Mutex, mpsc::Receiver},
    time::{Duration, Instant},
};

#[cfg(feature = "testing")]
mod testing;

type SampleArrays<'py> = (Bound<'py, PyArray1<f64>>, Bound<'py, PyArray1<f32>>);

create_exception!(_benchwatt, DeviceError, PyException);
create_exception!(_benchwatt, TimeoutError, PyTimeoutError);
create_exception!(_benchwatt, AlertError, PyException);

fn device_error(e: Error) -> PyErr {
    match e.kind {
        ErrorKind::Timeout => TimeoutError::new_err(e.message),
        ErrorKind::Invalid => PyValueError::new_err(e.message),
        _ => DeviceError::new_err(e.message),
    }
}
fn need<T>(cap: Option<T>, what: &str) -> PyResult<T> {
    cap.ok_or_else(|| device_error(Error::invalid(format!("device has no {what}"))))
}
fn file_error(e: std::io::Error) -> PyErr {
    pyo3::exceptions::PyOSError::new_err(e.to_string())
}
/// Plain Python dicts, lists and numbers from a device value or description.
fn to_py(py: Python<'_>, v: &Value) -> PyResult<Py<PyAny>> {
    Ok(py
        .import("json")?
        .call_method1("loads", (v.to_string(),))?
        .unbind())
}
fn from_py(v: &Bound<'_, PyAny>) -> PyResult<Value> {
    let text: String = v
        .py()
        .import("json")?
        .call_method1("dumps", (v,))?
        .extract()?;
    text.parse()
        .map_err(|e: serde_json::Error| PyValueError::new_err(e.to_string()))
}
fn bounds(duration: f64, t0: Option<f64>, t1: Option<f64>) -> PyResult<(f64, f64)> {
    let (from, to) = (t0.unwrap_or(0.0), t1.unwrap_or(duration));
    if !from.is_finite() || !to.is_finite() || from < 0.0 || to < from {
        return Err(PyValueError::new_err("invalid time range"));
    }
    Ok((from, to))
}
fn channel(rec: &NativeRecording, name: &str, kind: Kind) -> PyResult<u16> {
    rec.specs()
        .iter()
        .find(|s| s.name == name && s.kind == kind)
        .map(|s| s.id)
        .ok_or_else(|| {
            let kind = format!("{kind:?}").to_lowercase();
            PyValueError::new_err(format!("unknown {kind} channel {name}"))
        })
}

#[pyclass(name = "Recording", module = "benchwatt")]
struct PyRecording {
    rec: NativeRecording,
    meta: Meta,
}
#[pymethods]
impl PyRecording {
    #[staticmethod]
    fn load(path: &str) -> PyResult<Self> {
        let (rec, meta) = NativeRecording::load(path).map_err(file_error)?;
        Ok(Self { rec, meta })
    }
    #[getter]
    fn duration(&self) -> f64 {
        self.rec.duration()
    }
    #[getter]
    fn channels(&self) -> Vec<String> {
        self.rec.specs().into_iter().map(|s| s.name).collect()
    }
    #[getter]
    fn gaps(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let out = pyo3::types::PyList::empty(py);
        for g in self.rec.gaps() {
            let d = PyDict::new(py);
            d.set_item("from", g.from)?;
            d.set_item("to", g.to)?;
            out.append(d)?;
        }
        Ok(out.into_any().unbind())
    }
    #[pyo3(signature = (name, t0=None, t1=None))]
    fn data<'py>(
        &self,
        py: Python<'py>,
        name: &str,
        t0: Option<f64>,
        t1: Option<f64>,
    ) -> PyResult<SampleArrays<'py>> {
        let id = channel(&self.rec, name, Kind::Analog)?;
        let (from, to) = bounds(self.duration(), t0, t1)?;
        let (t, v) = self.rec.analog_data(id, from, to).unwrap();
        Ok((PyArray1::from_vec(py, t), PyArray1::from_vec(py, v)))
    }
    #[pyo3(signature = (name, t0=None, t1=None))]
    fn stats(
        &self,
        py: Python<'_>,
        name: &str,
        t0: Option<f64>,
        t1: Option<f64>,
    ) -> PyResult<Option<Py<PyAny>>> {
        let id = channel(&self.rec, name, Kind::Analog)?;
        let (from, to) = bounds(self.duration(), t0, t1)?;
        let Some(s) = self.rec.stats(id, from, to) else {
            return Ok(None);
        };
        let d = PyDict::new(py);
        for (key, value) in [
            ("mean", s.mean),
            ("min", s.min),
            ("max", s.max),
            ("rms", s.rms),
            ("pp", s.max - s.min),
            ("integral", s.integral),
        ] {
            d.set_item(key, value)?;
        }
        d.set_item("count", s.count)?;
        Ok(Some(d.into_any().unbind()))
    }
    #[pyo3(signature = (t0=None, t1=None))]
    fn charge_mah(&self, t0: Option<f64>, t1: Option<f64>) -> PyResult<Option<f64>> {
        let (from, to) = bounds(self.duration(), t0, t1)?;
        Ok(self.rec.charge_mah(from, to))
    }
    #[pyo3(signature = (t0=None, t1=None))]
    fn energy(
        &self,
        py: Python<'_>,
        t0: Option<f64>,
        t1: Option<f64>,
    ) -> PyResult<Option<Py<PyAny>>> {
        let (from, to) = bounds(self.duration(), t0, t1)?;
        let Some(e) = self.rec.energy(from, to) else {
            return Ok(None);
        };
        let d = PyDict::new(py);
        d.set_item("joules", e.joules)?;
        d.set_item("mwh", e.mwh)?;
        d.set_item("average_power", e.average_power)?;
        Ok(Some(d.into_any().unbind()))
    }
    #[pyo3(signature = (t0=None, t1=None))]
    fn edges(&self, py: Python<'_>, t0: Option<f64>, t1: Option<f64>) -> PyResult<Py<PyAny>> {
        let (from, to) = bounds(self.duration(), t0, t1)?;
        let out = pyo3::types::PyList::empty(py);
        for e in self.rec.edges(from, to) {
            let d = PyDict::new(py);
            d.set_item("t", e.t)?;
            d.set_item("mask", e.mask)?;
            out.append(d)?;
        }
        Ok(out.into_any().unbind())
    }
    #[pyo3(signature = (name, t0=None, t1=None))]
    fn bytes(
        &self,
        py: Python<'_>,
        name: &str,
        t0: Option<f64>,
        t1: Option<f64>,
    ) -> PyResult<Vec<(f64, Py<PyBytes>)>> {
        let id = channel(&self.rec, name, Kind::Bytes)?;
        let (from, to) = bounds(self.duration(), t0, t1)?;
        Ok(self
            .rec
            .bytes(id, from, to)
            .into_iter()
            .map(|u| (u.t, PyBytes::new(py, &u.bytes).unbind()))
            .collect())
    }
    fn save(&self, path: &str) -> PyResult<()> {
        self.rec.save(path, &self.meta).map_err(file_error)
    }
    fn export(&self, path: &str) -> PyResult<()> {
        match std::path::Path::new(path)
            .extension()
            .and_then(|x| x.to_str())
        {
            Some("csv") => export::export_csv(&self.rec, path, None).map_err(file_error),
            Some("sr") => export::export_sr(&self.rec, path).map_err(file_error),
            _ => Err(PyValueError::new_err("export path must end in .csv or .sr")),
        }
    }
}

#[pyclass(name = "Device", module = "benchwatt")]
struct PyDevice {
    dev: Option<Box<dyn Device>>,
    rx: Mutex<Receiver<Event>>,
    recording: Option<(Mutex<Capture>, NativeRecording)>,
    alerts: Vec<Value>,
    latest: HashMap<String, f32>,
}
impl PyDevice {
    fn new(dev: Box<dyn Device>) -> Self {
        let rx = dev.events();
        Self {
            dev: Some(dev),
            rx: Mutex::new(rx),
            recording: None,
            alerts: Vec::new(),
            latest: HashMap::new(),
        }
    }
    fn dev(&self) -> PyResult<&dyn Device> {
        self.dev
            .as_deref()
            .ok_or_else(|| DeviceError::new_err("device is closed"))
    }
    fn supply(&self) -> PyResult<&dyn Supply> {
        need(self.dev()?.supply(), "supply output")
    }
    /// The output a call acts on: the one named, or the only one there is.
    fn output_n(&self, out: Option<u8>) -> PyResult<(&dyn Supply, u8)> {
        let supply = self.supply()?;
        if let Some(n) = out {
            return Ok((supply, n));
        }
        match supply.outputs().as_slice() {
            [only] => Ok((supply, only.n)),
            all => {
                let labels: Vec<_> = all
                    .iter()
                    .map(|o| format!("{} ({})", o.n, o.label))
                    .collect();
                Err(PyValueError::new_err(format!(
                    "this supply has several outputs, pick one with out=: {}",
                    labels.join(", ")
                )))
            }
        }
    }
    fn state(&self, out: Option<u8>) -> PyResult<OutputState> {
        let (supply, n) = self.output_n(out)?;
        supply.state(n).map_err(device_error)
    }
    fn drain(&mut self) {
        let channels = self.dev.as_ref().map_or(Vec::new(), |d| d.channels());
        loop {
            let e = self.rx.lock().unwrap().try_recv();
            let reading = match e {
                Ok(Event::Alert {
                    id,
                    name,
                    title,
                    detail,
                    severity,
                }) => {
                    self.alerts
                        .push(json!({"id": id, "name": name, "title": title,
                        "detail": detail, "severity": severity}));
                    continue;
                }
                Ok(Event::Reading { ch, value }) => {
                    channels.iter().find(|c| c.id == ch).zip(Some(value))
                }
                Ok(Event::Inputs(mask)) => channels
                    .iter()
                    .find(|c| c.kind == Kind::Digital)
                    .zip(Some(mask as f32)),
                Ok(_) => None,
                Err(_) => break,
            };
            if let Some((c, value)) = reading {
                self.latest.insert(c.name.clone(), value);
            }
        }
    }
    fn finish(&mut self) -> PyResult<PyRecording> {
        let (capture, mut rec) = self
            .recording
            .take()
            .ok_or_else(|| PyValueError::new_err("recording is not active"))?;
        let dev = self.dev()?;
        let result = capture.into_inner().unwrap().stop(dev, &mut rec);
        let meta = benchwatt_session::meta(dev);
        self.drain();
        result.map_err(device_error)?;
        Ok(PyRecording { rec, meta })
    }
}
#[pymethods]
impl PyDevice {
    fn __enter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }
    fn __exit__(
        &mut self,
        _ty: &Bound<'_, PyAny>,
        _value: &Bound<'_, PyAny>,
        _tb: &Bound<'_, PyAny>,
    ) -> PyResult<()> {
        self.close()
    }
    fn close(&mut self) -> PyResult<()> {
        if self.recording.is_some() {
            let _ = self.finish();
        }
        self.dev.take();
        Ok(())
    }
    #[getter]
    fn info(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let i = self.dev()?.info();
        let mut out = serde_json::Map::new();
        for (key, value) in [
            ("kind", i.kind),
            ("model", i.model),
            ("serial", i.serial),
            ("firmware", i.firmware),
            ("hardware", i.hardware),
        ] {
            out.insert(key.into(), json!((!value.is_empty()).then_some(value)));
        }
        out.extend(i.details);
        to_py(py, &Value::Object(out))
    }
    #[getter]
    fn channels(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        to_py(py, &json!(self.dev()?.channels()))
    }
    #[getter]
    fn outputs(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let outputs = self.dev()?.supply().map_or(Vec::new(), |s| s.outputs());
        to_py(py, &json!(outputs))
    }
    #[getter]
    fn settings(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        to_py(py, &json!(self.dev()?.settings()))
    }
    fn get(&self, py: Python<'_>, key: &str) -> PyResult<Py<PyAny>> {
        to_py(py, &self.dev()?.get(key).map_err(device_error)?)
    }
    fn set(&self, key: &str, value: &Bound<'_, PyAny>) -> PyResult<()> {
        self.dev()?.set(key, from_py(value)?).map_err(device_error)
    }
    #[pyo3(signature = (volts, out=None))]
    fn set_voltage(&self, volts: f64, out: Option<u8>) -> PyResult<()> {
        let (supply, n) = self.output_n(out)?;
        supply.set_voltage(n, volts).map_err(device_error)
    }
    #[pyo3(signature = (amps, out=None))]
    fn set_current_limit(&self, amps: f64, out: Option<u8>) -> PyResult<()> {
        let (supply, n) = self.output_n(out)?;
        supply.set_current_limit(n, amps).map_err(device_error)
    }
    #[pyo3(signature = (on, out=None))]
    fn set_output(&self, on: bool, out: Option<u8>) -> PyResult<()> {
        let (supply, n) = self.output_n(out)?;
        supply.set_enabled(n, on).map_err(device_error)
    }
    #[pyo3(signature = (out=None))]
    fn output_state(&self, py: Python<'_>, out: Option<u8>) -> PyResult<Py<PyAny>> {
        to_py(py, &json!(self.state(out)?))
    }
    #[getter]
    fn output(&self) -> PyResult<bool> {
        Ok(self.state(None)?.enabled)
    }
    #[setter(output)]
    fn set_only_output(&self, on: bool) -> PyResult<()> {
        self.set_output(on, None)
    }
    #[getter]
    fn voltage(&self) -> PyResult<Option<f64>> {
        Ok(self.state(None)?.voltage)
    }
    #[getter]
    fn current_limit(&self) -> PyResult<Option<f64>> {
        Ok(self.state(None)?.current_limit)
    }
    fn set_gpo(&self, n: u8, on: bool) -> PyResult<()> {
        need(self.dev()?.gpio(), "digital outputs")?
            .set_output(n, on)
            .map_err(device_error)
    }
    fn monitor(&mut self) -> HashMap<String, f32> {
        self.drain();
        self.latest.clone()
    }
    fn alerts(&mut self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        self.drain();
        to_py(py, &Value::Array(std::mem::take(&mut self.alerts)))
    }
    fn uart_open(&self, baud: u32) -> PyResult<()> {
        need(self.dev()?.uart(), "UART")?
            .open(baud)
            .map_err(device_error)
    }
    /// Written bytes also go into the running recording, if there is one.
    fn uart_write(&mut self, data: &[u8]) -> PyResult<()> {
        let dev = self
            .dev
            .as_deref()
            .ok_or_else(|| DeviceError::new_err("device is closed"))?;
        let rec = self.recording.as_mut().map(|(_, rec)| rec);
        benchwatt_session::uart_write(dev, rec, data).map_err(device_error)
    }
    fn uart_close(&self) -> PyResult<()> {
        need(self.dev()?.uart(), "UART")?
            .close()
            .map_err(device_error)
    }
    /// Without channels: the analog channels that measure the outputs.
    #[pyo3(signature = (channels=None, digital=false, uart=None))]
    fn start_recording(
        &mut self,
        channels: Option<Vec<String>>,
        digital: bool,
        uart: Option<u32>,
    ) -> PyResult<()> {
        if self.recording.is_some() {
            return Err(PyValueError::new_err("recording already active"));
        }
        let dev = self.dev()?;
        let channels = channels.unwrap_or_else(|| {
            dev.channels()
                .into_iter()
                .filter(|c| c.kind == Kind::Analog && !c.group.is_empty())
                .map(|c| c.name)
                .collect()
        });
        let (capture, rec) = Capture::start(dev, &channels, digital, uart).map_err(device_error)?;
        self.recording = Some((Mutex::new(capture), rec));
        Ok(())
    }
    fn stop_recording(&mut self) -> PyResult<PyRecording> {
        self.finish()
    }
    #[pyo3(signature = (duration, channels=None, digital=false, uart=None))]
    fn record(
        &mut self,
        py: Python<'_>,
        duration: f64,
        channels: Option<Vec<String>>,
        digital: bool,
        uart: Option<u32>,
    ) -> PyResult<PyRecording> {
        if !duration.is_finite() || duration < 0.0 || duration > u64::MAX as f64 {
            return Err(PyValueError::new_err(
                "duration must be nonnegative, finite, and representable",
            ));
        }
        let wait_for = Duration::from_secs_f64(duration);
        let deadline = Instant::now()
            .checked_add(wait_for)
            .ok_or_else(|| PyValueError::new_err("duration is too large"))?;
        self.start_recording(channels, digital, uart)?;
        while Instant::now() < deadline {
            let wait = deadline
                .saturating_duration_since(Instant::now())
                .min(Duration::from_millis(50));
            let Some((capture, rec)) = &mut self.recording else {
                break;
            };
            let capture = capture.get_mut().unwrap();
            let alive = py.detach(move || capture.pump(rec, wait));
            self.drain();
            if let Err(e) = py.check_signals() {
                if !e.is_instance_of::<pyo3::exceptions::PyKeyboardInterrupt>(py) {
                    let _ = self.finish();
                    return Err(e);
                }
                break;
            }
            if !alive {
                break;
            }
        }
        self.finish()
    }
}

/// The supported devices that are plugged in.
#[pyfunction]
fn devices(py: Python<'_>) -> PyResult<Py<PyAny>> {
    let found: Vec<_> = benchwatt_session::find()
        .iter()
        .map(|f| json!({"kind": f.kind, "port": f.port}))
        .collect();
    to_py(py, &Value::Array(found))
}
/// Open a device: the only one plugged in, a kind, or `kind@port`.
#[pyfunction]
#[pyo3(signature = (device=None))]
fn open(device: Option<&str>) -> PyResult<PyDevice> {
    let found = benchwatt_session::select(device).map_err(device_error)?;
    Ok(PyDevice::new(
        benchwatt_session::open(&found).map_err(device_error)?,
    ))
}

#[pymodule]
fn _benchwatt(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyDevice>()?;
    m.add_class::<PyRecording>()?;
    m.add_function(wrap_pyfunction!(devices, m)?)?;
    m.add_function(wrap_pyfunction!(open, m)?)?;
    m.add("DeviceError", m.py().get_type::<DeviceError>())?;
    m.add("TimeoutError", m.py().get_type::<TimeoutError>())?;
    m.add("AlertError", m.py().get_type::<AlertError>())?;
    #[cfg(feature = "testing")]
    testing::register(m)?;
    Ok(())
}
