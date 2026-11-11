use benchwatt_session::{
    Capture,
    device::{Device, Error, ErrorKind, Event, Kind, OutputState, Value},
    rec::{Recording as NativeRecording, export, file::Meta},
};
use numpy::PyArray1;
use pyo3::{
    create_exception,
    exceptions::{PyException, PyTimeoutError, PyValueError},
    prelude::*,
    types::{PyBytes, PyDict},
};
use std::{
    collections::HashMap,
    sync::{Mutex, mpsc::Receiver},
    time::{Duration, Instant},
};

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
fn bounds(duration: f64, t0: Option<f64>, t1: Option<f64>) -> PyResult<(f64, f64)> {
    let (from, to) = (t0.unwrap_or(0.0), t1.unwrap_or(duration));
    if !from.is_finite() || !to.is_finite() || from < 0.0 || to < from {
        return Err(PyValueError::new_err("invalid time range"));
    }
    Ok((from, to))
}
fn channel(rec: &NativeRecording, name: &str) -> PyResult<u16> {
    rec.specs()
        .iter()
        .find(|s| s.name == name && s.kind == Kind::Analog)
        .map(|s| s.id)
        .ok_or_else(|| PyValueError::new_err(format!("unknown analog channel {name}")))
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
        let id = channel(&self.rec, name)?;
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
        let id = channel(&self.rec, name)?;
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
    #[pyo3(signature = (t0=None, t1=None))]
    fn uart(
        &self,
        py: Python<'_>,
        t0: Option<f64>,
        t1: Option<f64>,
    ) -> PyResult<Vec<(f64, Py<PyBytes>)>> {
        let (from, to) = bounds(self.duration(), t0, t1)?;
        let rx = self
            .rec
            .specs()
            .into_iter()
            .find(|s| s.name == "rx" && s.kind == Kind::Bytes);
        Ok(rx
            .map_or(Vec::new(), |s| self.rec.bytes(s.id, from, to))
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

#[pyclass(name = "Arc", module = "benchwatt")]
struct PyArc {
    dev: Option<Box<dyn Device>>,
    rx: Mutex<Receiver<Event>>,
    recording: Option<(Mutex<Capture>, NativeRecording)>,
    alerts: Vec<(u16, String)>,
    latest: HashMap<String, f32>,
}
impl PyArc {
    fn dev(&self) -> PyResult<&dyn Device> {
        self.dev
            .as_deref()
            .ok_or_else(|| DeviceError::new_err("Arc is closed"))
    }
    fn state(&self) -> PyResult<OutputState> {
        need(self.dev()?.supply(), "supply")?
            .state(1)
            .map_err(device_error)
    }
    fn drain(&mut self) {
        let channels = self.dev.as_ref().map_or(Vec::new(), |d| d.channels());
        loop {
            let e = self.rx.lock().unwrap().try_recv();
            match e {
                Ok(Event::Alert { id, name, .. }) => self.alerts.push((id, name)),
                Ok(Event::Reading { ch, value }) => {
                    if let Some(c) = channels.iter().find(|c| c.id == ch) {
                        self.latest.insert(c.name.clone(), value);
                    }
                }
                Ok(Event::Inputs(mask)) => {
                    self.latest.insert("gpi".into(), mask as f32);
                }
                Ok(_) => (),
                Err(_) => break,
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
impl PyArc {
    #[new]
    #[pyo3(signature = (port=None))]
    fn new(port: Option<&str>) -> PyResult<Self> {
        let arc = port.map_or("arc".into(), |p| format!("arc@{p}"));
        let found = benchwatt_session::select(Some(&arc))
            .map_err(|_| DeviceError::new_err("no Arc serial port found"))?;
        let dev = benchwatt_session::open(&found).map_err(device_error)?;
        let rx = dev.events();
        Ok(Self {
            dev: Some(dev),
            rx: Mutex::new(rx),
            recording: None,
            alerts: Vec::new(),
            latest: HashMap::new(),
        })
    }
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
        let dev = self.dev()?;
        let i = dev.info();
        let d = PyDict::new(py);
        for (name, value) in [
            ("name", i.model),
            ("fw", i.firmware),
            ("hw", i.hardware),
            ("uid", i.serial),
        ] {
            d.set_item(name, (!value.is_empty()).then_some(value))?;
        }
        d.set_item(
            "protocol",
            i.details.get("protocol").and_then(Value::as_u64),
        )?;
        let mut channels: Vec<_> = dev.channels().into_iter().map(|c| c.name).collect();
        channels.sort();
        d.set_item("channels", channels)?;
        Ok(d.into_any().unbind())
    }
    fn set_voltage(&self, volts: f64) -> PyResult<()> {
        need(self.dev()?.supply(), "supply")?
            .set_voltage(1, volts)
            .map_err(device_error)
    }
    fn set_current_limit(&self, amps: f64) -> PyResult<()> {
        need(self.dev()?.supply(), "supply")?
            .set_current_limit(1, amps)
            .map_err(device_error)
    }
    fn set_current_mode(&self, mode: &str) -> PyResult<()> {
        let id = match mode {
            "cutoff" => 0,
            "limit" => 1,
            _ => return Err(PyValueError::new_err("mode must be cutoff or limit")),
        };
        self.dev()?
            .set("mainsrccurrmode", id.into())
            .map_err(device_error)
    }
    #[getter]
    fn output(&self) -> PyResult<bool> {
        Ok(self.state()?.enabled)
    }
    #[setter]
    fn set_output(&self, on: bool) -> PyResult<()> {
        need(self.dev()?.supply(), "supply")?
            .set_enabled(1, on)
            .map_err(device_error)
    }
    #[getter]
    fn voltage(&self) -> PyResult<f64> {
        need(self.state()?.voltage, "voltage setpoint")
    }
    #[getter]
    fn current_limit(&self) -> PyResult<f64> {
        need(self.state()?.current_limit, "current limit")
    }
    fn set_gpo(&self, n: u8, on: bool) -> PyResult<()> {
        need(self.dev()?.gpio(), "GPIO")?
            .set_output(n, on)
            .map_err(device_error)
    }
    fn monitor(&mut self) -> HashMap<String, f32> {
        self.drain();
        self.latest.clone()
    }
    fn alerts(&mut self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        self.drain();
        let out = pyo3::types::PyList::empty(py);
        for (id, name) in self.alerts.drain(..) {
            let d = PyDict::new(py);
            d.set_item("id", id)?;
            d.set_item("name", name)?;
            out.append(d)?;
        }
        Ok(out.into_any().unbind())
    }
    fn uart_open(&self, baud: u32) -> PyResult<()> {
        need(self.dev()?.uart(), "UART")?
            .open(baud)
            .map_err(device_error)
    }
    fn uart_write(&self, bytes: &[u8]) -> PyResult<()> {
        need(self.dev()?.uart(), "UART")?
            .write(bytes)
            .map_err(device_error)
    }
    fn uart_close(&self) -> PyResult<()> {
        need(self.dev()?.uart(), "UART")?
            .close()
            .map_err(device_error)
    }
    #[pyo3(signature = (channels, gpi=false, uart=false))]
    fn start_recording(&mut self, channels: Vec<String>, gpi: bool, uart: bool) -> PyResult<()> {
        if self.recording.is_some() {
            return Err(PyValueError::new_err("recording already active"));
        }
        let (capture, rec) = Capture::start(self.dev()?, &channels, gpi, uart.then_some(115200))
            .map_err(device_error)?;
        self.recording = Some((Mutex::new(capture), rec));
        Ok(())
    }
    fn stop_recording(&mut self) -> PyResult<PyRecording> {
        self.finish()
    }
    #[pyo3(signature = (duration, channels=None, gpi=false, uart=false))]
    fn record(
        &mut self,
        py: Python<'_>,
        duration: f64,
        channels: Option<Vec<String>>,
        gpi: bool,
        uart: bool,
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
        self.start_recording(
            channels.unwrap_or_else(|| vec!["mc".into(), "mv".into(), "mp".into()]),
            gpi,
            uart,
        )?;
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

#[cfg(feature = "testing")]
#[pyfunction]
fn recording_from_fixture(path: &str) -> PyResult<PyRecording> {
    let input = std::fs::read_to_string(path).map_err(file_error)?;
    let (channels, blocks) = otii_arc::adapter::replay(&input);
    let keep: Vec<_> = channels
        .into_iter()
        .filter(|c| matches!(c.name.as_str(), "mc" | "mv" | "gpi" | "rx"))
        .collect();
    let mut rec = NativeRecording::new(&keep);
    for b in &blocks {
        rec.push(b);
    }
    rec.stop();
    Ok(PyRecording {
        rec,
        meta: Meta::default(),
    })
}

#[pymodule]
fn _benchwatt(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyArc>()?;
    m.add_class::<PyRecording>()?;
    m.add("DeviceError", m.py().get_type::<DeviceError>())?;
    m.add("TimeoutError", m.py().get_type::<TimeoutError>())?;
    m.add("AlertError", m.py().get_type::<AlertError>())?;
    #[cfg(feature = "testing")]
    {
        let testing = PyModule::new(m.py(), "_testing")?;
        testing.add_function(wrap_pyfunction!(recording_from_fixture, &testing)?)?;
        m.add_submodule(&testing)?;
    }
    Ok(())
}
