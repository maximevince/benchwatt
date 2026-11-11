//! Recording engine. A 4 kHz f32 stream uses about 58 MB/h of raw storage.
pub use benchwatt_device::{Block, ChannelDesc, Data, Kind, Quantity};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
const CHUNK: usize = 4096;
pub mod export;
pub mod file;
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
    fn push(&mut self, t: f64, dt: f64, v: &[f32]) {
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
            self.chunks.last_mut().unwrap().push(x);
            self.len += 1;
            self.add(
                0,
                Bucket {
                    t: t + i as f64 * dt,
                    end: t + (i + 1) as f64 * dt,
                    min: x,
                    max: x,
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
const UART_JOIN_S: f64 = 0.002;
const UART_CHUNK_MAX: usize = 256;
struct Channel {
    spec: ChannelDesc,
    a: Analog,
    gaps: Vec<Gap>,
    /// Where the previous non-empty block ended, for gap detection.
    end: Option<f64>,
    edges: Vec<Edge>,
    digital_count: usize,
    uart: Vec<UartChunk>,
    /// Arrival time of the newest rx bytes, for merging split bursts.
    uart_end: f64,
    last: Option<u32>,
}
impl Channel {
    fn new(spec: ChannelDesc) -> Self {
        Self {
            spec,
            a: Analog::default(),
            gaps: Vec::new(),
            end: None,
            edges: Vec::new(),
            digital_count: 0,
            uart: Vec::new(),
            uart_end: f64::NEG_INFINITY,
            last: None,
        }
    }
    /// Records a gap when a block starts more than half a sample after the previous one ended.
    fn follow(&mut self, t: f64, dt: f64, len: usize) {
        if let Some(end) = self.end
            && t - end > 0.5 * dt
        {
            self.gaps.push(Gap { from: end, to: t });
            self.a.gap();
        }
        self.end = Some(t + len as f64 * dt);
    }
}
pub struct Recording {
    channels: HashMap<u16, Channel>,
    /// Latest end of any stored block.
    end: f64,
    /// Start time of the newest stored block.
    newest: Option<f64>,
    active: bool,
}
impl Recording {
    pub fn new(specs: &[ChannelDesc]) -> Self {
        Self {
            channels: specs
                .iter()
                .map(|s| (s.id, Channel::new(s.clone())))
                .collect(),
            end: 0.0,
            newest: None,
            active: true,
        }
    }
    pub fn active(&self) -> bool {
        self.active
    }
    pub fn stop(&mut self) {
        self.active = false
    }
    pub fn specs(&self) -> Vec<ChannelDesc> {
        let mut v: Vec<_> = self.channels.values().map(|c| c.spec.clone()).collect();
        v.sort_by_key(|s| s.id);
        v
    }
    pub fn add_channel(&mut self, s: ChannelDesc) {
        self.channels.entry(s.id).or_insert_with(|| Channel::new(s));
    }
    /// Store a block; sample i is at `b.t + i * b.dt` on the recording's time axis.
    pub fn push(&mut self, b: &Block) {
        if !self.active {
            return;
        }
        let Some(c) = self.channels.get_mut(&b.ch) else {
            return;
        };
        let (t, dt) = (b.t, b.dt);
        let end = match (&c.spec.kind, &b.data) {
            (Kind::Analog, Data::Analog(v)) if !v.is_empty() => {
                c.follow(t, dt, v.len());
                c.a.push(t, dt, v);
                t + v.len() as f64 * dt
            }
            (Kind::Digital, Data::Digital(v)) => {
                if !v.is_empty() {
                    c.follow(t, dt, v.len());
                }
                c.digital_count += v.len();
                for (i, &mask) in v.iter().enumerate() {
                    if c.last != Some(mask) {
                        c.edges.push(Edge {
                            t: t + i as f64 * dt,
                            mask,
                        });
                        c.last = Some(mask)
                    }
                }
                t + v.len() as f64 * dt
            }
            (Kind::Bytes, Data::Bytes(v)) => {
                if !v.is_empty() {
                    // A burst may be split across consecutive blocks; join the pieces so a line stays whole.
                    let joins = t - c.uart_end <= UART_JOIN_S;
                    match c.uart.last_mut() {
                        Some(prev)
                            if joins
                                && prev.bytes.last() != Some(&b'\n')
                                && prev.bytes.len() < UART_CHUNK_MAX =>
                        {
                            prev.bytes.extend_from_slice(v)
                        }
                        _ => c.uart.push(UartChunk {
                            t,
                            bytes: v.clone(),
                        }),
                    }
                    c.uart_end = t;
                }
                t + dt
            }
            _ => return,
        };
        self.end = self.end.max(end);
        self.newest = Some(self.newest.map_or(t, |n| n.max(t)));
    }
    /// Append transmitted bytes on `ch` at the start time of the newest block.
    pub fn push_tx(&mut self, ch: &ChannelDesc, bytes: &[u8]) {
        let Some(t) = self.newest.filter(|_| self.active && !bytes.is_empty()) else {
            return;
        };
        self.add_channel(ch.clone());
        let c = self.channels.get_mut(&ch.id).unwrap();
        if c.spec.kind == Kind::Bytes {
            c.uart.push(UartChunk {
                t,
                bytes: bytes.to_vec(),
            });
        }
    }
    pub fn duration(&self) -> f64 {
        self.end
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
    /// Gaps of all channels, merged where they overlap.
    pub fn gaps(&self) -> Vec<Gap> {
        let mut all: Vec<Gap> = self
            .channels
            .values()
            .flat_map(|c| c.gaps.iter().copied())
            .collect();
        all.sort_by(|a, b| a.from.total_cmp(&b.from));
        let mut out: Vec<Gap> = Vec::with_capacity(all.len());
        for g in all {
            match out.last_mut() {
                Some(last) if g.from <= last.to + 1e-9 => last.to = last.to.max(g.to),
                _ => out.push(g),
            }
        }
        out
    }
    /// Native analog samples, without display gap markers or resampling.
    pub fn analog_data(&self, ch: u16, t0: f64, t1: f64) -> Option<(Vec<f64>, Vec<f32>)> {
        if !t0.is_finite() || !t1.is_finite() || t0 < 0.0 || t1 < t0 {
            return None;
        }
        let channel = self.channels.get(&ch)?;
        if channel.spec.kind != Kind::Analog {
            return None;
        }
        let a = &channel.a;
        let (lo, hi) = a.bounds(t0, t1);
        let mut times = Vec::with_capacity(hi - lo);
        let mut values = Vec::with_capacity(hi - lo);
        for i in lo..hi {
            times.push(a.time(i));
            values.push(a.value(i));
        }
        Some((times, values))
    }
    pub fn view(&self, ch: u16, t0: f64, t1: f64, px: usize) -> Option<View> {
        if !t0.is_finite() || !t1.is_finite() || t1 < t0 {
            return None;
        }
        let c = self.channels.get(&ch)?;
        let a = &c.a;
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
            let gap = c.gaps.partition_point(|g| g.from <= x.t);
            if let Some(g) = c.gaps.get(gap)
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
    /// The channel measuring `q` in the group of the lowest-id grouped channel.
    fn primary(&self, q: Quantity) -> Option<&Channel> {
        let specs = self.specs();
        let group = &specs.iter().find(|s| !s.group.is_empty())?.group;
        let s = specs
            .iter()
            .find(|s| &s.group == group && s.quantity == q)?;
        self.channels.get(&s.id)
    }
    /// Charge of the primary group; per-group results come with multi-device recordings.
    pub fn charge_mah(&self, t0: f64, t1: f64) -> Option<f64> {
        let ch = self.primary(Quantity::Current)?.spec.id;
        Some(self.stats(ch, t0, t1)?.integral / 3.6)
    }
    /// Energy of the primary group; per-group results come with multi-device recordings.
    pub fn energy(&self, t0: f64, t1: f64) -> Option<Energy> {
        let joules = if let Some(p) = self.primary(Quantity::Power).filter(|c| c.a.len > 0) {
            self.stats(p.spec.id, t0, t1)?.integral
        } else {
            let mc = &self.primary(Quantity::Current)?.a;
            let mv = &self.primary(Quantity::Voltage)?.a;
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
        let digital = self.specs().into_iter().find(|s| s.kind == Kind::Digital);
        digital.map_or(Vec::new(), |s| {
            let c = &self.channels[&s.id];
            let s = c.edges.partition_point(|x| x.t < t0);
            let e = c.edges.partition_point(|x| x.t <= t1);
            c.edges[s..e].to_vec()
        })
    }
    pub fn bytes(&self, ch: u16, t0: f64, t1: f64) -> Vec<UartChunk> {
        if !t0.is_finite() || !t1.is_finite() || t1 < t0 {
            return Vec::new();
        }
        self.channels.get(&ch).map_or(Vec::new(), |c| {
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
