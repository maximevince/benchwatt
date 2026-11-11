use benchwatt_device::{Block, Data, Device, ErrorKind, Event, Kind, Meter, Supply, Value};
use benchwatt_ppk2::{CH_D, CH_I, CH_V, Ppk2, convert, parse_calibration};
use std::{
    collections::VecDeque,
    io::{self, Read, Write},
    sync::{Arc, Mutex, mpsc},
    thread,
    time::{Duration, Instant},
};

const FIXTURE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/ppk2-calibration.txt"
));

type Respond = Box<dyn FnMut(&[u8]) -> Vec<Vec<u8>> + Send>;
struct State {
    written: Vec<u8>,
    queue: VecDeque<Vec<u8>>,
    respond: Respond,
    idle_reads: usize,
    broken: bool,
}
/// Both ends of an in-memory serial port; each queued chunk is one read.
#[derive(Clone)]
struct Port(Arc<Mutex<State>>);
impl Port {
    fn new(respond: impl FnMut(&[u8]) -> Vec<Vec<u8>> + Send + 'static) -> Self {
        Self(Arc::new(Mutex::new(State {
            written: Vec::new(),
            queue: VecDeque::new(),
            respond: Box::new(respond),
            idle_reads: 0,
            broken: false,
        })))
    }
    /// Answers calibration requests with the fixture and start with `stream`.
    fn device(stream: Vec<Vec<u8>>) -> Self {
        let mut stream = Some(stream);
        Self::new(move |cmd| match cmd {
            [0x19] => vec![FIXTURE.as_bytes().to_vec()],
            [0x06] => stream.take().unwrap_or_default(),
            _ => Vec::new(),
        })
    }
    fn written(&self) -> Vec<u8> {
        self.0.lock().unwrap().written.clone()
    }
    fn clear(&self) {
        self.0.lock().unwrap().written.clear();
    }
    /// Wait until the reader has taken everything queued and come back for more.
    fn drained(&self) {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut seen = None;
        while Instant::now() < deadline {
            let s = self.0.lock().unwrap();
            if s.queue.is_empty() {
                match seen {
                    Some(n) if s.idle_reads > n => return,
                    None => seen = Some(s.idle_reads),
                    _ => {}
                }
            }
            drop(s);
            thread::sleep(Duration::from_millis(1));
        }
        panic!("stream not drained");
    }
}
impl Read for Port {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let mut s = self.0.lock().unwrap();
        if s.broken {
            return Err(io::Error::new(io::ErrorKind::BrokenPipe, "unplugged"));
        }
        let Some(mut chunk) = s.queue.pop_front() else {
            s.idle_reads += 1;
            drop(s);
            thread::sleep(Duration::from_millis(2));
            return Err(io::ErrorKind::TimedOut.into());
        };
        let n = chunk.len().min(buf.len());
        buf[..n].copy_from_slice(&chunk[..n]);
        if n < chunk.len() {
            s.queue.push_front(chunk.split_off(n));
        }
        Ok(n)
    }
}
impl Write for Port {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut s = self.0.lock().unwrap();
        s.written.extend_from_slice(buf);
        let replies = (s.respond)(buf);
        s.queue.extend(replies);
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn word(raw: u16, range: u8, counter: u8, bits: u8) -> u32 {
    u32::from(raw) | u32::from(range) << 14 | u32::from(counter) << 18 | u32::from(bits) << 24
}
/// `n` samples with consecutive counters from `counter`, raw value = index.
fn words(n: usize, counter: u8) -> Vec<u32> {
    (0..n)
        .map(|k| {
            word(
                k as u16 & 0x3fff,
                1,
                (counter as usize + k) as u8 & 0x3f,
                k as u8,
            )
        })
        .collect()
}
/// Little-endian bytes cut into chunks of awkward sizes, so words split across reads.
fn chunks(words: &[u32]) -> Vec<Vec<u8>> {
    let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    let sizes = [3, 4097, 5, 2, 7000, 1];
    let (mut out, mut at, mut i) = (Vec::new(), 0, 0);
    while at < bytes.len() {
        let end = (at + sizes[i % sizes.len()]).min(bytes.len());
        out.push(bytes[at..end].to_vec());
        (at, i) = (end, i + 1);
    }
    out
}
fn open(port: &Port) -> Ppk2 {
    Ppk2::from_io(port.clone(), port.clone()).unwrap()
}
fn run(words: &[u32], channels: &[u16]) -> (Ppk2, Port, Vec<Block>) {
    let port = Port::device(chunks(words));
    let ppk = open(&port);
    ppk.set("spike_filter", false.into()).unwrap();
    let (tx, rx) = mpsc::channel();
    ppk.start(channels, tx).unwrap();
    port.drained();
    ppk.stop().unwrap();
    let blocks = rx.try_iter().collect();
    (ppk, port, blocks)
}
fn analog(b: &Block) -> &[f32] {
    match &b.data {
        Data::Analog(v) => v,
        d => panic!("not analog: {d:?}"),
    }
}

#[test]
fn open_stops_discards_and_reads_calibration() {
    let port = Port::device(Vec::new());
    // Left over from an unclean stop: stream bytes, and half a calibration reply.
    port.0
        .lock()
        .unwrap()
        .queue
        .extend([vec![0xAB; 4001], b"R0: 5\nEND\n".to_vec()]);
    let ppk = open(&port);
    assert_eq!(port.written(), [0x07, 0x19]);
    assert_eq!(*ppk.calibration(), parse_calibration(FIXTURE).unwrap());
    let info = ppk.info();
    assert_eq!((info.kind.as_str(), info.model.as_str()), ("ppk2", "PPK2"));
    assert_eq!(info.hardware, "6407");
    assert_eq!(info.details["VDD"], 3700);
    assert_eq!(info.details["Calibrated"], false);
    assert_eq!(info.details["mode"], 2);
    assert_eq!(info.details["IA"], 59.0);
    assert_eq!(ppk.get("mode").unwrap(), "source");
    let s = ppk.state(1).unwrap();
    assert_eq!((s.enabled, s.voltage, s.current_limit), (false, 3.7, 0.0));
}

#[test]
fn open_retries_a_cut_reply() {
    let mut first = true;
    let port = Port::new(move |cmd| match cmd {
        [0x19] if std::mem::take(&mut first) => vec![FIXTURE.as_bytes()[..500].to_vec()],
        [0x19] => vec![
            FIXTURE.as_bytes()[..700].to_vec(),
            FIXTURE.as_bytes()[700..].to_vec(),
        ],
        _ => Vec::new(),
    });
    let ppk = open(&port);
    assert_eq!(port.written(), [0x07, 0x19, 0x07, 0x19]);
    assert_eq!(ppk.calibration().r[4], 0.056347765028476715);
}

#[test]
fn open_fails_without_end() {
    let port = Port::new(|cmd| match cmd {
        [0x19] => vec![FIXTURE.as_bytes()[..900].to_vec()],
        _ => Vec::new(),
    });
    let e = Ppk2::from_io(port.clone(), port.clone()).err().unwrap();
    assert_eq!(e.kind, ErrorKind::Timeout);
    assert_eq!(port.written(), [0x07, 0x19, 0x07, 0x19, 0x07, 0x19]);
}

#[test]
fn describes_itself() {
    let ppk = open(&Port::device(Vec::new()));
    let ch = ppk.channels();
    let names: Vec<_> = ch
        .iter()
        .map(|c| (c.id, c.name.as_str(), c.rate_hz, c.kind))
        .collect();
    assert_eq!(
        names,
        [
            (CH_I, "i", 100_000, Kind::Analog),
            (CH_V, "v", 100, Kind::Analog),
            (CH_D, "d", 100_000, Kind::Digital)
        ]
    );
    assert!(ch[1].set && !ch[0].set);
    assert_eq!((ch[0].group.as_str(), ch[1].group.as_str()), ("out", "out"));
    assert_eq!(ch[2].bits, 8);
    let out = &ppk.outputs()[0];
    assert_eq!((out.voltage.min, out.voltage.max), (0.8, 5.0));
    assert!(out.current_limit.is_none());
    let keys: Vec<_> = ppk.settings().into_iter().map(|s| s.key).collect();
    assert_eq!(keys, ["mode", "spike_filter"]);
    assert_eq!(ppk.get("spike_filter").unwrap(), true);
}

#[test]
fn commands() {
    let port = Port::device(Vec::new());
    let ppk = open(&port);
    let events = ppk.events();
    port.clear();
    ppk.set_voltage(1, 3.3).unwrap();
    assert_eq!(port.written(), [0x0D, 0x0C, 0xE4]);
    assert_eq!(ppk.state(1).unwrap().voltage, 3.3);
    port.clear();
    ppk.set_enabled(1, true).unwrap();
    ppk.set_enabled(1, false).unwrap();
    ppk.set_enabled(1, true).unwrap();
    assert_eq!(port.written(), [0x0C, 1, 0x0C, 0, 0x0C, 1]);
    assert!(ppk.state(1).unwrap().enabled);
    port.clear();
    ppk.set("mode", "ampere".into()).unwrap();
    assert_eq!(port.written(), [0x11, 1, 0x0D, 0x0C, 0xE4]);
    assert_eq!(ppk.get("mode").unwrap(), "ampere");
    port.clear();
    ppk.set("mode", "source".into()).unwrap();
    assert_eq!(port.written(), [0x11, 2]);
    assert_eq!(
        events
            .try_iter()
            .filter(|e| matches!(e, Event::Changed))
            .count(),
        6
    );

    port.clear();
    assert_eq!(
        ppk.set_current_limit(1, 0.1).unwrap_err().kind,
        ErrorKind::Invalid
    );
    assert!(ppk.set_voltage(1, 0.7).is_err());
    assert!(ppk.set_voltage(1, 5.01).is_err());
    assert!(ppk.set_voltage(2, 3.0).is_err());
    assert!(ppk.set("mode", "sink".into()).is_err());
    assert!(ppk.set("mode", Value::Bool(true)).is_err());
    assert!(ppk.set("speed", 1.into()).is_err());
    assert!(port.written().is_empty());

    let (tx, _rx) = mpsc::channel();
    ppk.start(&[CH_I], tx.clone()).unwrap();
    assert!(ppk.start(&[CH_I], tx).is_err());
    ppk.stop().unwrap();
    ppk.stop().unwrap();
    assert_eq!(port.written(), [0x06, 0x07]);
    assert!(ppk.start(&[7], mpsc::channel().0).is_err());
    // Dropping stops a running stream but leaves the output alone.
    port.clear();
    ppk.start(&[CH_I], mpsc::channel().0).unwrap();
    drop(ppk);
    assert_eq!(port.written(), [0x06, 0x07]);
}

#[test]
fn blocks_follow_the_sample_counter() {
    let w = words(2500, 17);
    let (ppk, _, blocks) = run(&w, &[CH_I, CH_D]);
    let cal = ppk.calibration();
    let i: Vec<_> = blocks.iter().filter(|b| b.ch == CH_I).collect();
    assert_eq!(i.iter().map(|b| b.t).collect::<Vec<_>>(), [0.0, 0.01, 0.02]);
    assert_eq!(
        i.iter().map(|b| analog(b).len()).collect::<Vec<_>>(),
        [1000, 1000, 500]
    );
    assert!(i.iter().all(|b| b.dt == 1e-5));
    let got: Vec<f32> = i.iter().flat_map(|b| analog(b).to_vec()).collect();
    let want: Vec<f32> = w
        .iter()
        .map(|&x| convert(x, cal, 3700.0).amps as f32)
        .collect();
    assert_eq!(got, want);
    let d: Vec<u32> = blocks
        .iter()
        .filter(|b| b.ch == CH_D)
        .flat_map(|b| match &b.data {
            Data::Digital(v) => v.clone(),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(d, (0..2500u32).map(|k| k & 0xff).collect::<Vec<_>>());
    assert!(!blocks.iter().any(|b| b.ch == CH_V), "v was not requested");
}

#[test]
fn counter_jump_leaves_a_gap() {
    // 1500 samples, then 5 lost, then 1200 more.
    let mut w = words(1500, 0);
    w.extend(words(1200, ((1500 + 5) % 64) as u8));
    let (_, _, blocks) = run(&w, &[CH_I, CH_V]);
    let i: Vec<_> = blocks
        .iter()
        .filter(|b| b.ch == CH_I)
        .map(|b| (b.t, analog(b).len()))
        .collect();
    let at = |k: u64| k as f64 / 100_000.0;
    assert_eq!(
        i,
        [
            (0.0, 1000),
            (at(1000), 500),
            (at(1505), 1000),
            (at(2505), 200)
        ]
    );
    let v: Vec<_> = blocks
        .iter()
        .filter(|b| b.ch == CH_V)
        .map(|b| (b.t, b.dt, analog(b).to_vec()))
        .collect();
    assert_eq!(v.len(), 4);
    assert_eq!(v[0], (0.0, 0.01, vec![3.7]));
    assert_eq!(v[1], (at(1000), 0.005, vec![3.7]));
    assert!(!blocks.iter().any(|b| b.ch == CH_D));
}

#[test]
fn readings_and_set_voltage_while_streaming() {
    let port = Port::device(Vec::new());
    let ppk = open(&port);
    ppk.set("spike_filter", false.into()).unwrap();
    let events = ppk.events();
    let (tx, rx) = mpsc::channel();
    ppk.start(&[CH_V], tx).unwrap();
    // Range 1 raw 0 is −4.369511975345025e-6 A at 3.7 V (see tests/proto.rs).
    let zero: Vec<u32> = (0..1000).map(|k| word(0, 1, (k % 64) as u8, 0)).collect();
    port.0.lock().unwrap().queue.extend(chunks(&zero));
    port.drained();
    ppk.set_voltage(1, 1.8).unwrap();
    let more: Vec<u32> = (1000..2000)
        .map(|k| word(0, 1, (k % 64) as u8, 0))
        .collect();
    port.0.lock().unwrap().queue.extend(chunks(&more));
    port.drained();
    ppk.stop().unwrap();
    let v: Vec<_> = rx
        .try_iter()
        .map(|b| (b.ch, b.t, analog(&b).to_vec()))
        .collect();
    assert_eq!(v, [(CH_V, 0.0, vec![3.7]), (CH_V, 0.01, vec![1.8])]);
    let readings: Vec<f32> = events
        .try_iter()
        .filter_map(|e| match e {
            Event::Reading { ch: CH_I, value } => Some(value),
            _ => None,
        })
        .collect();
    assert_eq!(readings.len(), 2);
    assert!((readings[0] - -4.369_512e-6).abs() < 1e-12, "{readings:?}");
}

#[test]
fn disconnect_while_streaming() {
    let port = Port::device(Vec::new());
    let ppk = open(&port);
    let events = ppk.events();
    let (tx, rx) = mpsc::channel();
    ppk.start(&[CH_I], tx).unwrap();
    port.0.lock().unwrap().broken = true;
    assert!(matches!(
        events.recv_timeout(Duration::from_secs(2)),
        Ok(Event::Disconnected)
    ));
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(2)),
        Err(mpsc::RecvTimeoutError::Disconnected)
    );
}
