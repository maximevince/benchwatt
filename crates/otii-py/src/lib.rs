use numpy::PyArray1;
use otii_arc::{Arc as Device, Event, PropertyValue, prop};
use otii_rec::{ChannelSpec, Kind, Recording as NativeRecording, export, file::Meta};
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

create_exception!(_openotii, DeviceError, PyException);
create_exception!(_openotii, TimeoutError, PyTimeoutError);
create_exception!(_openotii, AlertError, PyException);

fn device_error(e: otii_arc::Error) -> PyErr {
    match e {
        otii_arc::Error::Timeout => TimeoutError::new_err(e.to_string()),
        otii_arc::Error::Property(_) => PyValueError::new_err(e.to_string()),
        _ => DeviceError::new_err(e.to_string()),
    }
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
fn specs(dev: &Device, names: &[String], gpi: bool, uart: bool) -> PyResult<Vec<ChannelSpec>> {
    let mut ids = Vec::new();
    for name in names {
        let id = prop::channel_by_name(name)
            .ok_or_else(|| PyValueError::new_err(format!("unknown channel {name}")))?;
        ids.push(id);
    }
    if gpi {
        ids.push(0x16);
    }
    if uart {
        ids.push(0x17);
    }
    ids.sort_unstable();
    ids.dedup();
    if ids.is_empty() {
        return Err(PyValueError::new_err("no channels selected"));
    }
    let info = dev.info();
    Ok(ids
        .into_iter()
        .map(|id| {
            let kind = match id {
                0x16 => Kind::Digital,
                0x17 | 0x19 => Kind::Bytes,
                _ => Kind::Analog,
            };
            let unit = prop::CHANNELS
                .iter()
                .find(|c| c.0 == id)
                .map_or("", |c| c.2);
            ChannelSpec {
                id,
                name: prop::channel_name(id).into(),
                unit: unit.into(),
                rate_hz: info.chinfo.get(id).rate_hz,
                kind,
            }
        })
        .collect())
}

#[pyclass(name = "Recording", module = "openotii")]
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
        Ok(self
            .rec
            .uart(from, to)
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

#[pyclass(name = "Arc", module = "openotii")]
struct PyArc {
    dev: Option<Device>,
    rx: Mutex<Receiver<Event>>,
    recording: Option<NativeRecording>,
    alerts: Vec<(u16, String)>,
    latest: HashMap<String, f32>,
    capture_uart: bool,
}
impl PyArc {
    fn dev(&self) -> PyResult<&Device> {
        self.dev
            .as_ref()
            .ok_or_else(|| DeviceError::new_err("Arc is closed"))
    }
    fn collect(&mut self, event: Event) {
        match event {
            Event::Data { ms, block } => {
                if let Some(rec) = &mut self.recording {
                    rec.push_block(ms, &block)
                }
            }
            Event::Alert { id, name } => self.alerts.push((id, name.into())),
            Event::Monitor { ch, value } => {
                let name = prop::channel_name(ch as u16);
                if !name.is_empty() {
                    self.latest.insert(name.into(), value);
                }
            }
            Event::Gpi(mask) => {
                self.latest.insert("gpi".into(), mask as f32);
            }
            _ => (),
        }
    }
    fn drain(&mut self) {
        loop {
            let e = self.rx.lock().unwrap().try_recv();
            match e {
                Ok(e) => self.collect(e),
                Err(_) => break,
            }
        }
    }
    fn finish(&mut self) -> PyResult<PyRecording> {
        let result = self.dev()?.stop_recording().map_err(device_error);
        self.drain();
        let mut rec = self
            .recording
            .take()
            .ok_or_else(|| PyValueError::new_err("recording is not active"))?;
        rec.stop();
        if self.capture_uart {
            self.capture_uart = false;
            let _ = self.dev()?.uart_close();
        }
        result?;
        let info = self.dev()?.info();
        let mut meta = Meta::default();
        let get = |id| info.properties.get(&id).map(ToString::to_string);
        meta.device.name = get(1);
        meta.device.fw = get(0x81);
        meta.device.hw = get(0x80);
        meta.device.uid = get(0x82);
        Ok(PyRecording { rec, meta })
    }
}
#[pymethods]
impl PyArc {
    #[new]
    #[pyo3(signature = (port=None))]
    fn new(port: Option<&str>) -> PyResult<Self> {
        let path = port
            .map(str::to_owned)
            .or_else(otii_arc::device::find_port)
            .ok_or_else(|| DeviceError::new_err("no Arc serial port found"))?;
        let dev = Device::open(&path).map_err(device_error)?;
        let rx = dev.events();
        Ok(Self {
            dev: Some(dev),
            rx: Mutex::new(rx),
            recording: None,
            alerts: Vec::new(),
            latest: HashMap::new(),
            capture_uart: false,
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
        let i = self.dev()?.info();
        let d = PyDict::new(py);
        for (name, id) in [("name", 1), ("fw", 0x81), ("hw", 0x80), ("uid", 0x82)] {
            d.set_item(name, i.properties.get(&id).map(ToString::to_string))?;
        }
        d.set_item("protocol", i.proto)?;
        let mut channels: Vec<_> = i
            .chinfo
            .0
            .keys()
            .map(|id| prop::channel_name(*id).to_string())
            .filter(|s| !s.is_empty())
            .collect();
        channels.sort();
        d.set_item("channels", channels)?;
        Ok(d.into_any().unbind())
    }
    fn set_voltage(&self, volts: f64) -> PyResult<()> {
        self.dev()?.set_voltage(volts).map_err(device_error)
    }
    fn set_current_limit(&self, amps: f64) -> PyResult<()> {
        self.dev()?.set_current_limit(amps).map_err(device_error)
    }
    fn set_current_mode(&self, mode: &str) -> PyResult<()> {
        let id = match mode {
            "cutoff" => 0,
            "limit" => 1,
            _ => return Err(PyValueError::new_err("mode must be cutoff or limit")),
        };
        self.dev()?
            .set(6, &PropertyValue::Enum(id))
            .map_err(device_error)
    }
    #[getter]
    fn output(&self) -> PyResult<bool> {
        match self.dev()?.get_value(9).map_err(device_error)? {
            PropertyValue::Bool(v) => Ok(v),
            _ => Err(DeviceError::new_err("invalid output response")),
        }
    }
    #[setter]
    fn set_output(&self, on: bool) -> PyResult<()> {
        self.dev()?.output(on).map_err(device_error)
    }
    #[getter]
    fn voltage(&self) -> PyResult<f64> {
        match self.dev()?.get_value(11).map_err(device_error)? {
            PropertyValue::Number(v) => Ok(v),
            _ => Err(DeviceError::new_err("invalid voltage response")),
        }
    }
    #[getter]
    fn current_limit(&self) -> PyResult<f64> {
        match self.dev()?.get_value(12).map_err(device_error)? {
            PropertyValue::Number(v) => Ok(v),
            _ => Err(DeviceError::new_err("invalid current limit response")),
        }
    }
    fn set_gpo(&self, n: u8, on: bool) -> PyResult<()> {
        self.dev()?.set_gpo(n, on).map_err(device_error)
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
        self.dev()?.uart_open(baud).map_err(device_error)
    }
    fn uart_write(&self, bytes: &[u8]) -> PyResult<()> {
        self.dev()?.uart_write(bytes).map_err(device_error)
    }
    fn uart_close(&self) -> PyResult<()> {
        self.dev()?.uart_close().map_err(device_error)
    }
    #[pyo3(signature = (channels, gpi=false, uart=false))]
    fn start_recording(&mut self, channels: Vec<String>, gpi: bool, uart: bool) -> PyResult<()> {
        if self.recording.is_some() {
            return Err(PyValueError::new_err("recording already active"));
        }
        let selected = specs(self.dev()?, &channels, gpi, uart)?;
        self.drain();
        let ids: Vec<_> = selected.iter().map(|s| u32::from(s.id)).collect();
        if uart {
            self.dev()?.uart_open(115200).map_err(device_error)?;
        }
        if let Err(e) = self.dev()?.start_recording(&ids) {
            if uart {
                let _ = self.dev()?.uart_close();
            }
            return Err(device_error(e));
        }
        self.capture_uart = uart;
        self.recording = Some(NativeRecording::new(&selected));
        Ok(())
    }
    fn stop_recording(&mut self) -> PyResult<PyRecording> {
        if self.recording.is_none() {
            return Err(PyValueError::new_err("recording is not active"));
        }
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
            let event = py.detach(|| self.rx.lock().unwrap().recv_timeout(wait));
            if let Ok(e) = event {
                self.collect(e);
            }
            if let Err(e) = py.check_signals() {
                if !e.is_instance_of::<pyo3::exceptions::PyKeyboardInterrupt>(py) {
                    let _ = self.finish();
                    return Err(e);
                }
                break;
            }
        }
        self.finish()
    }
}

#[cfg(feature = "testing")]
#[pyfunction]
fn recording_from_fixture(path: &str) -> PyResult<PyRecording> {
    use otii_arc::msg::{self, Message};
    let input = std::fs::read_to_string(path).map_err(file_error)?;
    let mut table = otii_arc::prop::ChannelTable::default();
    let selected = [
        (0, "mc", "A", 4000),
        (1, "mv", "V", 1000),
        (0x16, "gpi", "", 1000),
        (0x17, "rx", "", 0),
    ];
    let specs: Vec<_> = selected
        .into_iter()
        .map(|(id, name, unit, rate)| ChannelSpec {
            id,
            name: name.into(),
            unit: unit.into(),
            rate_hz: rate,
            kind: match id {
                0x16 => Kind::Digital,
                0x17 => Kind::Bytes,
                _ => Kind::Analog,
            },
        })
        .collect();
    let mut rec = NativeRecording::new(&specs);
    for line in input
        .lines()
        .filter(|s| !s.is_empty() && !s.starts_with('#'))
    {
        if line.len() % 2 != 0 {
            return Err(PyValueError::new_err("odd fixture hex length"));
        }
        let bytes: Result<Vec<_>, _> = (0..line.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&line[i..i + 2], 16))
            .collect();
        let msg = msg::parse(
            &bytes.map_err(|e| PyValueError::new_err(e.to_string()))?,
            &table,
        )
        .map_err(|e| PyValueError::new_err(format!("{e:?}")))?;
        match msg {
            Message::CmdResp { txn: 14, body, .. } => {
                table = otii_arc::prop::ChannelTable::decode(&body)
            }
            Message::Data { ms, blocks } => {
                for block in blocks {
                    rec.push_block(ms, &block);
                }
            }
            _ => (),
        }
    }
    rec.stop();
    Ok(PyRecording {
        rec,
        meta: Meta::default(),
    })
}

#[pymodule]
fn _openotii(m: &Bound<'_, PyModule>) -> PyResult<()> {
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
