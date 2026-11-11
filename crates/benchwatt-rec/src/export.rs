//! CSV and sigrok session exporters. UART is intentionally absent from srzip.
use super::{Kind, Recording};
use std::{
    fs::File,
    io::{self, BufWriter, Write},
    path::Path,
};
use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};
fn rate(rec: &Recording) -> io::Result<u32> {
    rec.specs()
        .iter()
        .filter(|s| s.kind == Kind::Analog)
        .map(|s| s.rate_hz)
        .max()
        .filter(|&r| r > 0)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no analog channels"))
}
fn span(rec: &Recording, range: Option<(f64, f64)>, hz: u32) -> io::Result<(u64, u64)> {
    let (from, to) = range.unwrap_or((0.0, rec.duration()));
    if !from.is_finite() || !to.is_finite() || from < 0.0 || to < from {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid time range",
        ));
    }
    Ok((
        (from * hz as f64).ceil() as u64,
        (to.min(rec.duration()) * hz as f64).ceil() as u64,
    ))
}
pub fn export_csv(
    rec: &Recording,
    path: impl AsRef<Path>,
    range: Option<(f64, f64)>,
) -> io::Result<()> {
    let hz = rate(rec)?;
    let (first, end) = span(rec, range, hz)?;
    let specs: Vec<_> = rec
        .specs()
        .into_iter()
        .filter(|s| s.kind == Kind::Analog)
        .collect();
    let mut out = BufWriter::new(File::create(path)?);
    write!(out, "time_s")?;
    for s in &specs {
        write!(out, ",{}[{}]", s.name, s.unit)?;
    }
    writeln!(out)?;
    for i in first..end {
        let t = i as f64 / hz as f64;
        write!(out, "{t:.9}")?;
        for s in &specs {
            if let Some(v) = rec.analog_at(s.id, t) {
                write!(out, ",{v}")?;
            } else {
                write!(out, ",")?;
            }
        }
        writeln!(out)?;
    }
    out.flush()
}
pub fn export_csv_channel(
    rec: &Recording,
    ch: u16,
    path: impl AsRef<Path>,
    range: Option<(f64, f64)>,
) -> io::Result<()> {
    if !rec
        .specs()
        .iter()
        .any(|s| s.id == ch && s.kind == Kind::Analog)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not an analog channel",
        ));
    }
    let (from, to) = range.unwrap_or((0.0, rec.duration()));
    if !from.is_finite() || !to.is_finite() || from < 0.0 || to < from {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid time range",
        ));
    }
    let mut out = BufWriter::new(File::create(path)?);
    writeln!(out, "time_s,value")?;
    if let Some(super::View::Raw { t, v }) = rec.view(ch, from, to, usize::MAX / 2) {
        for (t, v) in t.into_iter().zip(v) {
            if !v.is_nan() {
                writeln!(out, "{t:.9},{v}")?;
            }
        }
    }
    out.flush()
}
pub fn export_sr(rec: &Recording, path: impl AsRef<Path>) -> io::Result<()> {
    export_sr_range(rec, path, None)
}
const GPI_PROBES: usize = 2;

pub fn export_sr_range(
    rec: &Recording,
    path: impl AsRef<Path>,
    range: Option<(f64, f64)>,
) -> io::Result<()> {
    let hz = rate(rec)?;
    let (first, end) = span(rec, range, hz)?;
    let analog: Vec<_> = rec
        .specs()
        .into_iter()
        .filter(|s| s.kind == Kind::Analog)
        .collect();
    let logic = rec
        .specs()
        .iter()
        .any(|s| s.kind == Kind::Digital && s.id == 0x16);
    // The Arc has two digital inputs; the other mask bits aren't real inputs.
    let probes = if logic { GPI_PROBES } else { 0 };
    // Keys and analog numbering follow libsigrok's src/output/srzip.c, zip_create()
    // and zip_append_analog(): https://github.com/sigrokproject/libsigrok/blob/master/src/output/srzip.c
    let mut metadata = format!(
        "[global]\nsigrok version=0.5.2\n\n[device 1]\nsamplerate={hz} Hz\ntotal analog={}\nunitsize=1\n",
        analog.len()
    );
    if logic {
        metadata.push_str(&format!("capturefile=logic-1\ntotal probes={GPI_PROBES}\n"));
        for i in 1..=GPI_PROBES {
            metadata.push_str(&format!("probe{i}=gpi{i}\n"));
        }
    }
    for (i, s) in analog.iter().enumerate() {
        metadata.push_str(&format!("analog{}={}\n", probes + i + 1, s.name));
    }
    let mut zip = ZipWriter::new(File::create(path)?);
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    zip.start_file("version", options)
        .map_err(io::Error::other)?;
    zip.write_all(b"2")?;
    zip.start_file("metadata", options)
        .map_err(io::Error::other)?;
    zip.write_all(metadata.as_bytes())?;
    for (i, s) in analog.iter().enumerate() {
        zip.start_file(format!("analog-1-{}-1", probes + i + 1), options)
            .map_err(io::Error::other)?;
        for n in first..end {
            let t = n as f64 / hz as f64;
            zip.write_all(&rec.analog_at(s.id, t).unwrap_or(f32::NAN).to_le_bytes())?;
        }
    }
    if logic {
        zip.start_file("logic-1-1", options)
            .map_err(io::Error::other)?;
        let edges = rec.edges(0.0, rec.duration());
        let mut pos = 0;
        let mut mask = 0;
        for n in first..end {
            let t = n as f64 / hz as f64;
            while pos < edges.len() && edges[pos].t <= t + 1e-12 {
                mask = edges[pos].mask;
                pos += 1;
            }
            zip.write_all(&[(mask & ((1 << GPI_PROBES) - 1)) as u8])?;
        }
    }
    zip.finish().map_err(io::Error::other)?;
    Ok(())
}
