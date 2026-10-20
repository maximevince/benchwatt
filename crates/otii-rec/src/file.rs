//! Streaming native recording archive. Packet boundaries preserve exact sample timing.
use super::{ChannelSpec, Edge, Gap, Kind, Packet, Recording, UartChunk};
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::{self, Read, Write},
    path::Path,
};
use zip::{CompressionMethod, ZipArchive, ZipWriter, write::SimpleFileOptions};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub name: Option<String>,
    pub fw: Option<String>,
    pub hw: Option<String>,
    pub uid: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChannelMeta {
    #[serde(flatten)]
    pub spec: ChannelSpec,
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
fn safe_name(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}
#[derive(Serialize, Deserialize)]
struct UartIndex {
    t: f64,
    offset: u64,
    len: usize,
}
impl Recording {
    pub fn save(&self, path: impl AsRef<Path>, supplied: &Meta) -> io::Result<()> {
        let mut meta = supplied.clone();
        meta.format_version = 1;
        meta.gaps = self.gaps.clone();
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
        for c in self.channels.values() {
            for u in &c.uart {
                zip.write_all(&u.bytes)?;
                index.push(UartIndex {
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
        let meta: Meta = serde_json::from_reader(zip.by_name("meta.json").map_err(zip_err)?)?;
        if meta.format_version != 1 {
            return Err(invalid("unsupported recording version"));
        }
        let specs: Vec<_> = meta.channels.iter().map(|c| c.spec.clone()).collect();
        let mut rec = Self::new(&specs);
        rec.gaps = meta.gaps.clone();
        if meta.duration_s > 0.0 {
            rec.first = Some(0);
            rec.last = Some((meta.duration_s * 1000.0).round().max(1.0) as u32 - 1);
        }
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
                    values.push(f32::from_le_bytes(b) as f64);
                }
                a.push(p.t, &values);
                let last = a.packets.last_mut().unwrap();
                *last = Packet { dt: p.dt, ..*last };
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
            let c = rec
                .channels
                .get_mut(&0x17)
                .ok_or_else(|| invalid("UART channel missing"))?;
            let mut bin = zip.by_name("uart.bin").map_err(zip_err)?;
            let mut offset = 0;
            for item in index {
                if item.offset != offset {
                    return Err(invalid("UART index offset mismatch"));
                }
                let mut bytes = vec![0; item.len];
                bin.read_exact(&mut bytes)?;
                c.uart.push(UartChunk { t: item.t, bytes });
                offset += item.len as u64;
            }
        }
        rec.stop();
        Ok((rec, meta))
    }
}
