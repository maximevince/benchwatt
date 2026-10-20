//! Recording engine. A 4 kHz f32 stream uses about 58 MB/h of raw storage.
use otii_arc::msg::{Block, BlockBody};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
const CHUNK: usize = 4096;
pub mod export;
pub mod file;
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum Kind {
    Analog,
    Digital,
    Bytes,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChannelSpec {
    pub id: u16,
    pub name: String,
    pub unit: String,
    pub rate_hz: u32,
    pub kind: Kind,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub struct Gap {
    pub from: f64,
    pub to: f64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Edge {
    pub t: f64,
    pub mask: u32,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UartChunk {
    pub t: f64,
    pub bytes: Vec<u8>,
}
#[derive(Clone, Debug, Serialize)]
pub struct Stats {
    pub count: usize,
    pub min: f64,
    pub max: f64,
    pub mean: f64,
    pub rms: f64,
    pub integral: f64,
}
#[derive(Clone, Debug, Serialize)]
pub struct Energy {
    pub joules: f64,
    pub mwh: f64,
    pub average_power: f64,
}
#[derive(Clone, Debug)]
pub enum View {
    Raw {
        t: Vec<f64>,
        v: Vec<f32>,
    },
    Envelope {
        t: Vec<f64>,
        min: Vec<f32>,
        max: Vec<f32>,
    },
}
#[derive(Clone, Copy)]
struct Packet {
    first: usize,
    len: usize,
    t: f64,
    dt: f64,
}
#[derive(Clone, Copy)]
struct Bucket {
    t: f64,
    end: f64,
    min: f32,
    max: f32,
}
#[derive(Default)]
struct Analog {
    chunks: Vec<Vec<f32>>,
    len: usize,
    packets: Vec<Packet>,
    levels: Vec<Vec<Bucket>>,
    pending: Vec<Vec<Bucket>>,
}
impl Analog {
    fn value(&self, i: usize) -> f32 {
        self.chunks[i / CHUNK][i % CHUNK]
    }
    fn packet(&self, i: usize) -> Packet {
        self.packets[self.packets.partition_point(|p| p.first <= i) - 1]
    }
    fn time(&self, i: usize) -> f64 {
        let p = self.packet(i);
        p.t + (i - p.first) as f64 * p.dt
    }
    fn at(&self, t: f64) -> Option<f32> {
        let n = self.packets.partition_point(|p| p.t <= t);
        if n == 0 {
            return None;
        }
        let p = self.packets[n - 1];
        let i = ((t - p.t) / p.dt).floor().max(0.0) as usize;
        (i < p.len).then(|| self.value(p.first + i))
    }
    fn add(&mut self, l: usize, b: Bucket) {
        if self.levels.len() <= l {
            self.levels.resize_with(l + 1, Vec::new);
            self.pending.resize_with(l + 1, Vec::new);
        }
        // Level zero is the chunked f32 storage; keep only its unfinished
        // group here instead of duplicating every raw sample as a Bucket.
        if l > 0 {
            self.levels[l].push(b);
        }
        self.pending[l].push(b);
        if self.pending[l].len() == 16 {
            let g = std::mem::take(&mut self.pending[l]);
            self.add(
                l + 1,
                Bucket {
                    t: g[0].t,
                    end: g[g.len() - 1].end,
                    min: g.iter().fold(f32::INFINITY, |a, b| a.min(b.min)),
                    max: g.iter().fold(f32::NEG_INFINITY, |a, b| a.max(b.max)),
                },
            );
        }
    }
    fn gap(&mut self) {
        // Finish only the raw group. Higher levels may combine buckets from
        // either side of a gap; they remain independent columns in a view.
        if !self.pending.is_empty() && !self.pending[0].is_empty() {
            let group = std::mem::take(&mut self.pending[0]);
            self.add(
                1,
                Bucket {
                    t: group[0].t,
                    end: group[group.len() - 1].end,
                    min: group.iter().fold(f32::INFINITY, |a, b| a.min(b.min)),
                    max: group.iter().fold(f32::NEG_INFINITY, |a, b| a.max(b.max)),
                },
            );
        }
    }
    fn tail(&self, l: usize) -> Option<Bucket> {
        let mut all = self.pending.iter().take(l).flat_map(|v| v.iter());
        let first = *all.next()?;
        Some(all.fold(first, |a, b| Bucket {
            t: a.t.min(b.t),
            end: a.end.max(b.end),
            min: a.min.min(b.min),
            max: a.max.max(b.max),
        }))
    }
    fn push(&mut self, t: f64, v: &[f64]) {
        let dt = 0.001 / v.len() as f64;
        self.packets.push(Packet {
            first: self.len,
            len: v.len(),
            t,
            dt,
        });
        for (i, &x) in v.iter().enumerate() {
            if self.chunks.last().is_none_or(|c| c.len() == CHUNK) {
                self.chunks.push(Vec::with_capacity(CHUNK))
            }
            self.chunks.last_mut().unwrap().push(x as f32);
            self.len += 1;
            self.add(
                0,
                Bucket {
                    t: t + i as f64 * dt,
                    end: t + (i + 1) as f64 * dt,
                    min: x as f32,
                    max: x as f32,
                },
            )
        }
    }
    fn bounds(&self, t0: f64, t1: f64) -> (usize, usize) {
        let s = self
            .packets
            .partition_point(|p| p.t + p.len as f64 * p.dt < t0);
        let e = self.packets.partition_point(|p| p.t <= t1);
        let lo = self.packets.get(s).map_or(self.len, |p| {
            p.first + (((t0 - p.t) / p.dt).ceil() as isize).clamp(0, p.len as isize) as usize
        });
        let hi = if e == 0 {
            0
        } else {
            let p = self.packets[e - 1];
            p.first + (((t1 - p.t) / p.dt).floor() as isize + 1).clamp(0, p.len as isize) as usize
        };
        (lo.min(self.len), hi.min(self.len).max(lo.min(self.len)))
    }
}
struct Channel {
    spec: ChannelSpec,
    a: Analog,
    edges: Vec<Edge>,
    digital_count: usize,
    uart: Vec<UartChunk>,
    last: Option<u32>,
}
pub struct Recording {
    channels: HashMap<u16, Channel>,
    first: Option<u32>,
    last: Option<u32>,
    gaps: Vec<Gap>,
    active: bool,
}
impl Recording {
    pub fn new(specs: &[ChannelSpec]) -> Self {
        Self {
            channels: specs
                .iter()
                .map(|s| {
                    (
                        s.id,
                        Channel {
                            spec: s.clone(),
                            a: Analog::default(),
                            edges: Vec::new(),
                            digital_count: 0,
                            uart: Vec::new(),
                            last: None,
                        },
                    )
                })
                .collect(),
            first: None,
            last: None,
            gaps: Vec::new(),
            active: true,
        }
    }
    pub fn active(&self) -> bool {
        self.active
    }
    pub fn stop(&mut self) {
        self.active = false
    }
    pub fn specs(&self) -> Vec<ChannelSpec> {
        let mut v: Vec<_> = self.channels.values().map(|c| c.spec.clone()).collect();
        v.sort_by_key(|s| s.id);
        v
    }
    pub fn add_channel(&mut self, s: ChannelSpec) {
        self.channels.entry(s.id).or_insert_with(|| Channel {
            spec: s,
            a: Analog::default(),
            edges: Vec::new(),
            digital_count: 0,
            uart: Vec::new(),
            last: None,
        });
    }
    pub fn push_block(&mut self, ms: u32, b: &Block) {
        if !self.active {
            return;
        }
        let first = *self.first.get_or_insert(ms);
        if let Some(last) = self.last {
            let jump = ms.wrapping_sub(last);
            if jump > 1 && jump < 0x8000_0000 {
                self.gaps.push(Gap {
                    from: last.wrapping_sub(first).wrapping_add(1) as f64 / 1000.0,
                    to: ms.wrapping_sub(first) as f64 / 1000.0,
                });
                for c in self.channels.values_mut() {
                    c.a.gap()
                }
            }
        }
        self.last = Some(ms);
        let Some(c) = self.channels.get_mut(&b.ch) else {
            return;
        };
        let t = ms.wrapping_sub(first) as f64 / 1000.0;
        match (&c.spec.kind, &b.body) {
            (Kind::Analog, BlockBody::Samples(v)) if !v.is_empty() => c.a.push(t, v),
            (Kind::Digital, BlockBody::Digital(v)) => {
                c.digital_count += v.len();
                for (i, &mask) in v.iter().enumerate() {
                    if c.last != Some(mask) {
                        c.edges.push(Edge {
                            t: t + i as f64 / v.len() as f64 / 1000.0,
                            mask,
                        });
                        c.last = Some(mask)
                    }
                }
            }
            (Kind::Bytes, BlockBody::Bytes(v)) if !v.is_empty() => c.uart.push(UartChunk {
                t,
                bytes: v.clone(),
            }),
            _ => {}
        }
    }
    pub fn duration(&self) -> f64 {
        self.last
            .zip(self.first)
            .map_or(0.0, |(l, f)| l.wrapping_sub(f) as f64 / 1000.0 + 0.001)
    }
    pub fn sample_counts(&self) -> HashMap<String, usize> {
        self.channels
            .values()
            .map(|c| {
                (
                    c.spec.name.clone(),
                    match c.spec.kind {
                        Kind::Analog => c.a.len,
                        Kind::Digital => c.digital_count,
                        Kind::Bytes => c.uart.iter().map(|x| x.bytes.len()).sum(),
                    },
                )
            })
            .collect()
    }
    pub fn gaps(&self) -> &[Gap] {
        &self.gaps
    }
    pub fn view(&self, ch: u16, t0: f64, t1: f64, px: usize) -> Option<View> {
        if !t0.is_finite() || !t1.is_finite() || t1 < t0 {
            return None;
        }
        let a = &self.channels.get(&ch)?.a;
        let (lo, hi) = a.bounds(t0, t1);
        let budget = px.max(1).saturating_mul(2);
        if hi - lo <= budget {
            let mut t = Vec::with_capacity(hi - lo);
            let mut v = Vec::with_capacity(hi - lo);
            if lo < hi {
                let mut packet = a.packets.partition_point(|p| p.first <= lo) - 1;
                let mut previous: Option<(f64, f64)> = None;
                for i in lo..hi {
                    if i >= a.packets[packet].first + a.packets[packet].len {
                        packet += 1;
                    }
                    let p = a.packets[packet];
                    let at = p.t + (i - p.first) as f64 * p.dt;
                    if let Some((before, dt)) = previous
                        && at - before > dt * 1.5
                    {
                        t.push(before + dt);
                        v.push(f32::NAN);
                    }
                    t.push(at);
                    v.push(a.value(i));
                    previous = Some((at, p.dt));
                }
            }
            return Some(View::Raw { t, v });
        }
        if a.levels.len() == 1 {
            let mut min = f32::INFINITY;
            let mut max = f32::NEG_INFINITY;
            for i in lo..hi {
                let value = a.value(i);
                min = min.min(value);
                max = max.max(value);
            }
            return Some(View::Envelope {
                t: vec![a.time(lo)],
                min: vec![min],
                max: vec![max],
            });
        }
        let mut l = 1;
        loop {
            let completed = a.levels.get(l).map_or(0, |level| {
                level.partition_point(|b| b.t <= t1) - level.partition_point(|b| b.end <= t0)
            });
            let partial = usize::from(a.tail(l).is_some_and(|b| b.end > t0 && b.t <= t1));
            if completed + partial <= budget.saturating_add(1) || l >= a.levels.len() {
                break;
            }
            l += 1;
        }
        let b = a.levels.get(l).map_or(&[][..], Vec::as_slice);
        let s = b.partition_point(|x| x.end <= t0);
        let e = b.partition_point(|x| x.t <= t1);
        let mut t = Vec::new();
        let mut min = Vec::new();
        let mut max = Vec::new();
        let mut selected = b[s..e].to_vec();
        if let Some(tail) = a.tail(l)
            && tail.end > t0
            && tail.t <= t1
        {
            selected.push(tail);
        }
        for (i, x) in selected.iter().enumerate() {
            t.push(x.t);
            min.push(x.min);
            max.push(x.max);
            let next = selected.get(i + 1).map_or(t1, |b| b.t);
            let gap = self.gaps.partition_point(|g| g.from <= x.t);
            if let Some(g) = self.gaps.get(gap)
                && g.from < next
                && g.from <= t1
            {
                t.push(g.from);
                min.push(f32::NAN);
                max.push(f32::NAN);
            }
        }
        Some(View::Envelope { t, min, max })
    }
    pub fn stats(&self, ch: u16, t0: f64, t1: f64) -> Option<Stats> {
        if !t0.is_finite() || !t1.is_finite() || t1 < t0 {
            return None;
        }
        let a = &self.channels.get(&ch)?.a;
        let (lo, hi) = a.bounds(t0, t1);
        if lo == hi {
            return None;
        }
        let (mut min, mut max, mut sum, mut sq, mut integral) =
            (f64::INFINITY, f64::NEG_INFINITY, 0.0, 0.0, 0.0);
        let mut packet = a.packets.partition_point(|p| p.first <= lo) - 1;
        for i in lo..hi {
            if i >= a.packets[packet].first + a.packets[packet].len {
                packet += 1;
            }
            let v = a.value(i) as f64;
            min = min.min(v);
            max = max.max(v);
            sum += v;
            sq += v * v;
            integral += v * a.packets[packet].dt
        }
        let count = hi - lo;
        Some(Stats {
            count,
            min,
            max,
            mean: sum / count as f64,
            rms: (sq / count as f64).sqrt(),
            integral,
        })
    }
    pub fn charge_mah(&self, t0: f64, t1: f64) -> Option<f64> {
        Some(self.stats(0, t0, t1)?.integral / 3.6)
    }
    pub fn energy(&self, t0: f64, t1: f64) -> Option<Energy> {
        let joules = if self.channels.get(&6).is_some_and(|c| c.a.len > 0) {
            self.stats(6, t0, t1)?.integral
        } else {
            let mc = &self.channels.get(&0)?.a;
            let mv = &self.channels.get(&1)?.a;
            let (lo, hi) = mc.bounds(t0, t1);
            let mut j = 0.0;
            let mut found = false;
            for i in lo..hi {
                if let Some(v) = mv.at(mc.time(i)) {
                    j += mc.value(i) as f64 * v as f64 * mc.packet(i).dt;
                    found = true
                }
            }
            if !found {
                return None;
            }
            j
        };
        Some(Energy {
            joules,
            mwh: joules / 3.6,
            average_power: joules / (t1 - t0).max(f64::EPSILON),
        })
    }
    pub fn edges(&self, t0: f64, t1: f64) -> Vec<Edge> {
        if !t0.is_finite() || !t1.is_finite() || t1 < t0 {
            return Vec::new();
        }
        self.channels.get(&0x16).map_or(Vec::new(), |c| {
            let s = c.edges.partition_point(|x| x.t < t0);
            let e = c.edges.partition_point(|x| x.t <= t1);
            c.edges[s..e].to_vec()
        })
    }
    pub fn uart(&self, t0: f64, t1: f64) -> Vec<UartChunk> {
        if !t0.is_finite() || !t1.is_finite() || t1 < t0 {
            return Vec::new();
        }
        self.channels.get(&0x17).map_or(Vec::new(), |c| {
            let s = c.uart.partition_point(|x| x.t < t0);
            let e = c.uart.partition_point(|x| x.t <= t1);
            c.uart[s..e].to_vec()
        })
    }
}

impl Recording {
    /// Value held at a time, limited to the packet containing that time.
    pub fn analog_at(&self, ch: u16, t: f64) -> Option<f32> {
        let channel = self.channels.get(&ch)?;
        (channel.spec.kind == Kind::Analog)
            .then(|| channel.a.at(t))
            .flatten()
    }
}
