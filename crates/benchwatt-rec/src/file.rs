//! Streaming native recording archive. Packet boundaries preserve exact sample timing.
use super::{ChannelDesc, Edge, Gap, Kind, Recording, UartChunk};
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::{self, Read, Write},
    path::Path,
};
use zip::{CompressionMethod, ZipArchive, ZipWriter, write::SimpleFileOptions};

/// Which device a recording came from, as far as the device could say.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DeviceInfo {
    pub kind: Option<String>,
    pub model: Option<String>,
    pub serial: Option<String>,
    pub firmware: Option<String>,
    pub hardware: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChannelMeta {
    #[serde(flatten)]
    pub spec: ChannelDesc,
    pub sample_count: usize,
    pub first_sample_time: Option<f64>,
    packets: Vec<PacketMeta>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct PacketMeta {
    len: usize,
    t: f64,
    dt: f64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Meta {
    pub format_version: u32,
    pub created: String,
    pub app_version: String,
    pub device: DeviceInfo,
    pub channels: Vec<ChannelMeta>,
    pub gaps: Vec<Gap>,
    pub notes: Option<String>,
    pub duration_s: f64,
}
impl Default for Meta {
    fn default() -> Self {
        Self {
            format_version: 1,
            created: chrono::Utc::now().to_rfc3339(),
            app_version: env!("CARGO_PKG_VERSION").into(),
            device: DeviceInfo::default(),
            channels: vec![],
            gaps: vec![],
            notes: None,
            duration_s: 0.0,
        }
    }
}
fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
fn zip_err(e: zip::result::ZipError) -> io::Error {
    io::Error::other(e)
}
fn options() -> SimpleFileOptions {
    SimpleFileOptions::default().compression_method(CompressionMethod::Deflated)
}
/// Channel names become file names in the archive: letters, digits, `_`, `-`
/// and single dots inside the name (`ch1.v`), nothing that could leave `ch/`.
fn safe_name(s: &str) -> bool {
    !s.is_empty()
        && !s.starts_with('.')
        && !s.ends_with('.')
        && !s.contains("..")
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
}
#[derive(Serialize, Deserialize)]
struct UartIndex {
    ch: u16,
    t: f64,
    offset: u64,
    len: usize,
}
impl Recording {
    pub fn save(&self, path: impl AsRef<Path>, supplied: &Meta) -> io::Result<()> {
        let mut meta = supplied.clone();
        meta.format_version = 1;
        meta.gaps = self.gaps();
        meta.duration_s = self.duration();
        meta.channels = self
            .specs()
            .into_iter()
            .map(|spec| {
                let c = &self.channels[&spec.id];
                ChannelMeta {
                    sample_count: match spec.kind {
                        Kind::Analog => c.a.len,
                        Kind::Digital => c.digital_count,
                        Kind::Bytes => c.uart.iter().map(|u| u.bytes.len()).sum(),
                    },
                    first_sample_time: match spec.kind {
                        Kind::Analog => c.a.packets.first().map(|p| p.t),
                        Kind::Digital => c.edges.first().map(|e| e.t),
                        Kind::Bytes => c.uart.first().map(|u| u.t),
                    },
                    packets: c
                        .a
                        .packets
                        .iter()
                        .map(|p| PacketMeta {
                            len: p.len,
                            t: p.t,
                            dt: p.dt,
                        })
                        .collect(),
                    spec,
                }
            })
            .collect();
        let mut zip = ZipWriter::new(File::create(path)?);
        zip.start_file("meta.json", options()).map_err(zip_err)?;
        serde_json::to_writer(&mut zip, &meta)?;
        for channel in &meta.channels {
            if channel.spec.kind != Kind::Analog {
                continue;
            }
            if !safe_name(&channel.spec.name) {
                return Err(invalid("unsafe channel name"));
            }
            zip.start_file(format!("ch/{}.f32", channel.spec.name), options())
                .map_err(zip_err)?;
            for chunk in &self.channels[&channel.spec.id].a.chunks {
                for &value in chunk {
                    zip.write_all(&value.to_le_bytes())?;
                }
            }
        }
        let edges: Vec<(u16, Edge)> = self
            .channels
            .iter()
            .flat_map(|(&id, c)| c.edges.iter().cloned().map(move |e| (id, e)))
            .collect();
        zip.start_file("edges.json", options()).map_err(zip_err)?;
        serde_json::to_writer(&mut zip, &edges)?;
        let mut index = Vec::new();
        zip.start_file("uart.bin", options()).map_err(zip_err)?;
        let mut offset = 0;
        for (&ch, c) in &self.channels {
            for u in &c.uart {
                zip.write_all(&u.bytes)?;
                index.push(UartIndex {
                    ch,
                    t: u.t,
                    offset,
                    len: u.bytes.len(),
                });
                offset += u.bytes.len() as u64;
            }
        }
        zip.start_file("uart.json", options()).map_err(zip_err)?;
        serde_json::to_writer(&mut zip, &index)?;
        zip.finish().map_err(zip_err)?;
        Ok(())
    }
    pub fn load(path: impl AsRef<Path>) -> io::Result<(Self, Meta)> {
        let mut zip = ZipArchive::new(File::open(path)?).map_err(zip_err)?;
        let mut meta: Meta = serde_json::from_reader(zip.by_name("meta.json").map_err(zip_err)?)?;
        if meta.format_version != 1 {
            return Err(invalid("unsupported recording version"));
        }
        meta.gaps.sort_by(|a, b| a.from.total_cmp(&b.from));
        let specs: Vec<_> = meta.channels.iter().map(|c| c.spec.clone()).collect();
        let mut rec = Self::new(&specs);
        for c in rec.channels.values_mut() {
            if c.spec.kind != Kind::Bytes {
                c.gaps = meta.gaps.clone();
            }
        }
        rec.end = meta.duration_s.max(0.0);
        for cm in &meta.channels {
            if cm.spec.kind != Kind::Analog {
                continue;
            }
            if !safe_name(&cm.spec.name) {
                return Err(invalid("unsafe channel name"));
            }
            let mut entry = zip
                .by_name(&format!("ch/{}.f32", cm.spec.name))
                .map_err(zip_err)?;
            if entry.size() != cm.sample_count as u64 * 4 {
                return Err(invalid("sample count mismatch"));
            }
            let a = &mut rec
                .channels
                .get_mut(&cm.spec.id)
                .ok_or_else(|| invalid("channel missing"))?
                .a;
            for p in &cm.packets {
                if p.len == 0 || !p.t.is_finite() || !p.dt.is_finite() || p.dt <= 0.0 {
                    return Err(invalid("invalid packet timing"));
                }
                if a.packets
                    .last()
                    .is_some_and(|last| meta.gaps.iter().any(|g| g.from >= last.t && g.to <= p.t))
                {
                    a.gap();
                }
                let mut values = Vec::with_capacity(p.len);
                for _ in 0..p.len {
                    let mut b = [0; 4];
                    entry.read_exact(&mut b)?;
                    values.push(f32::from_le_bytes(b));
                }
                a.push(p.t, p.dt, &values);
            }
            if a.len != cm.sample_count {
                return Err(invalid("packet count mismatch"));
            }
        }
        let edges: Vec<(u16, Edge)> =
            serde_json::from_reader(zip.by_name("edges.json").map_err(zip_err)?)?;
        for (id, e) in edges {
            let c = rec
                .channels
                .get_mut(&id)
                .ok_or_else(|| invalid("edge channel missing"))?;
            c.last = Some(e.mask);
            c.edges.push(e);
        }
        for cm in &meta.channels {
            if cm.spec.kind == Kind::Digital {
                rec.channels
                    .get_mut(&cm.spec.id)
                    .ok_or_else(|| invalid("digital channel missing"))?
                    .digital_count = cm.sample_count;
            }
        }
        let index: Vec<UartIndex> =
            serde_json::from_reader(zip.by_name("uart.json").map_err(zip_err)?)?;
        if !index.is_empty() {
            let mut bin = zip.by_name("uart.bin").map_err(zip_err)?;
            let mut offset = 0;
            for item in index {
                if item.offset != offset {
                    return Err(invalid("UART index offset mismatch"));
                }
                let mut bytes = vec![0; item.len];
                bin.read_exact(&mut bytes)?;
                let c = rec
                    .channels
                    .get_mut(&item.ch)
                    .ok_or_else(|| invalid("UART channel missing"))?;
                if c.spec.kind != Kind::Bytes {
                    return Err(invalid("UART channel kind mismatch"));
                }
                c.uart.push(UartChunk { t: item.t, bytes });
                offset += item.len as u64;
            }
        }
        rec.stop();
        Ok((rec, meta))
    }
}

#[cfg(test)]
mod tests {
    use super::safe_name;
    #[test]
    fn channel_names_as_file_names() {
        for good in ["mc", "ch1.v", "adc-1_x"] {
            assert!(safe_name(good), "{good}");
        }
        for bad in ["", ".v", "v.", "a..b", "../x", "a/b", "a b"] {
            assert!(!safe_name(bad), "{bad}");
        }
    }
}
