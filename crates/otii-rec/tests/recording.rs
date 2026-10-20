use otii_arc::msg::{self, Block, BlockBody, Message};
use otii_arc::prop::ChannelTable;
use otii_rec::{ChannelSpec, Kind, Recording, View};
fn spec(id: u16, name: &str, kind: Kind, rate: u32) -> ChannelSpec {
    ChannelSpec {
        id,
        name: name.into(),
        unit: String::new(),
        rate_hz: rate,
        kind,
    }
}
fn block(ch: u16, body: BlockBody) -> Block {
    let n = match &body {
        BlockBody::Samples(v) => v.len(),
        BlockBody::Digital(v) => v.len(),
        BlockBody::Bytes(v) => v.len(),
    };
    Block {
        ch,
        n: n as u16,
        rate_hz: 4000,
        body,
    }
}
fn feed(r: &mut Recording, ms: u32, ch: u16, v: Vec<f64>) {
    r.push_block(ms, &block(ch, BlockBody::Samples(v)))
}
#[test]
fn charge_and_energy() {
    let mut r = Recording::new(&[
        spec(0, "mc", Kind::Analog, 4000),
        spec(1, "mv", Kind::Analog, 1000),
        spec(6, "mp", Kind::Analog, 4000),
    ]);
    for ms in 0..2000 {
        feed(&mut r, ms, 0, vec![0.0184; 4]);
        feed(&mut r, ms, 1, vec![3.3]);
        feed(&mut r, ms, 6, vec![0.06072; 4]);
    }
    let s = r.stats(0, 0.0, 2.0).unwrap();
    assert_eq!(s.count, 8000);
    assert!((s.integral - 0.0368).abs() < 1e-8);
    assert!((s.rms - 0.0184).abs() < 1e-8);
    assert!((r.charge_mah(0.0, 2.0).unwrap() - 0.0368 / 3.6).abs() < 1e-8);
    assert!((r.energy(0.0, 2.0).unwrap().joules - 0.12144).abs() < 1e-7);
}
#[test]
fn pyramid_and_view_bounds() {
    let mut r = Recording::new(&[spec(0, "mc", Kind::Analog, 4000)]);
    let mut expected = Vec::new();
    for ms in 0..10000 {
        let v = (0..4)
            .map(|i| {
                let t = (ms * 4 + i) as f64 / 4000.0;
                (t * std::f64::consts::TAU * 31.0).sin() + if ms > 5000 { 0.4 } else { 0.0 }
            })
            .collect::<Vec<_>>();
        expected.extend_from_slice(&v);
        feed(&mut r, ms, 0, v);
    }
    for (t0, t1, px) in [(0.0, 10.0, 300), (2.0, 2.8, 100), (0.7, 0.73, 60)] {
        match r.view(0, t0, t1, px).unwrap() {
            View::Raw { t, v } => {
                assert!(v.len() <= 2 * px);
                for (time, value) in t.iter().zip(v) {
                    assert!(
                        (expected[(time * 4000.0).round() as usize] - value as f64).abs() < 1e-5
                    )
                }
            }
            View::Envelope { t, min, max } => {
                assert!(t.len() <= 2 * px + 16);
                let bucket = if t1 - t0 > 1.0 { 256 } else { 16 };
                for ((time, lo), hi) in t.into_iter().zip(min).zip(max) {
                    let start = (time * 4000.0).round() as usize;
                    let slice = &expected[start..(start + bucket).min(expected.len())];
                    let brute_min = slice.iter().copied().fold(f64::INFINITY, f64::min);
                    let brute_max = slice.iter().copied().fold(f64::NEG_INFINITY, f64::max);
                    assert!(
                        (brute_min - lo as f64).abs() < 1e-4,
                        "range {t0}..{t1} time={time} bucket={bucket} got={lo} expected={brute_min}"
                    );
                    assert!(
                        (brute_max - hi as f64).abs() < 1e-4,
                        "range {t0}..{t1} time={time} bucket={bucket} got={hi} expected={brute_max}"
                    );
                }
            }
        }
    }
}
#[test]
fn gaps_and_edges() {
    let mut r = Recording::new(&[
        spec(0, "mc", Kind::Analog, 4000),
        spec(0x16, "gpi", Kind::Digital, 1000),
        spec(0x17, "rx", Kind::Bytes, 0),
    ]);
    feed(&mut r, 100, 0, vec![1.0; 4]);
    r.push_block(100, &block(0x16, BlockBody::Digital(vec![0, 1, 1])));
    feed(&mut r, 110, 0, vec![2.0; 4]);
    r.push_block(110, &block(0x16, BlockBody::Digital(vec![1, 0])));
    r.push_block(110, &block(0x17, BlockBody::Bytes(b"tick\n".to_vec())));
    assert_eq!(r.gaps().len(), 1);
    assert!(matches!(r.view(0, 0.003, 0.008, 10), Some(View::Raw { t, .. }) if t.is_empty()));
    assert!(
        matches!(r.view(0, 0.0, 0.02, 10), Some(View::Raw { v, .. }) if v.iter().any(|x| x.is_nan()))
    );
    assert_eq!(r.edges(0.0, 1.0).len(), 3);
    assert_eq!(r.uart(0.0, 1.0)[0].bytes, b"tick\n");
    assert_eq!(r.sample_counts()["mc"], 8);
}
#[test]
fn step_extrema_and_fallback_energy() {
    let mut r = Recording::new(&[
        spec(0, "mc", Kind::Analog, 4000),
        spec(1, "mv", Kind::Analog, 1000),
    ]);
    for ms in 0..1000 {
        feed(
            &mut r,
            ms,
            0,
            vec![if ms % 100 == 0 { 0.03 } else { 0.00005 }; 4],
        );
        feed(&mut r, ms, 1, vec![3.3]);
    }
    match r.view(0, 0.0, 1.0, 20).unwrap() {
        View::Envelope { min, max, .. } => {
            assert!(min.iter().any(|&x| (x - 0.00005).abs() < 1e-8));
            assert!(max.iter().any(|&x| (x - 0.03).abs() < 1e-8));
        }
        _ => panic!("expected envelope"),
    }
    let charge = r.stats(0, 0.0, 1.0).unwrap().integral;
    assert!((r.energy(0.0, 1.0).unwrap().joules - charge * 3.3).abs() < 1e-7);
}
#[test]
fn tiny_recording_still_respects_pixel_budget() {
    let mut r = Recording::new(&[spec(0, "mc", Kind::Analog, 4000)]);
    feed(&mut r, 0, 0, vec![1.0, 2.0, 3.0, 4.0]);
    feed(&mut r, 1, 0, vec![5.0, 6.0, 7.0, 8.0]);
    assert!(
        matches!(r.view(0, 0.0, 0.002, 1), Some(View::Envelope { min, max, .. }) if min == vec![1.0] && max == vec![8.0])
    );
}
#[test]
fn many_short_runs_keep_the_pyramid_bounded() {
    let mut r = Recording::new(&[spec(0, "mc", Kind::Analog, 4000)]);
    for i in 0..100 {
        feed(&mut r, i * 3, 0, vec![i as f64; 4]);
    }
    assert_eq!(r.gaps().len(), 99);
    match r.view(0, 0.0, 1.0, 1).unwrap() {
        View::Envelope { t, min, max } => {
            assert!(t.len() <= 3, "too many buckets: {}", t.len());
            assert_eq!(min.iter().copied().fold(f32::INFINITY, f32::min), 0.0);
            assert_eq!(max.iter().copied().fold(f32::NEG_INFINITY, f32::max), 99.0);
        }
        _ => panic!("expected envelope"),
    }
}
fn hex(s: &str) -> Vec<u8> {
    s.as_bytes()
        .chunks_exact(2)
        .map(|p| u8::from_str_radix(std::str::from_utf8(p).unwrap(), 16).unwrap())
        .collect()
}
#[test]
fn captured_session() {
    let mut table = ChannelTable::default();
    let mut r = Recording::new(&[
        spec(0, "mc", Kind::Analog, 4000),
        spec(1, "mv", Kind::Analog, 1000),
    ]);
    for line in include_str!("../../../fixtures/session1.hex")
        .lines()
        .filter(|s| !s.is_empty() && !s.starts_with('#'))
    {
        match msg::parse(&hex(line), &table).unwrap() {
            Message::CmdResp { txn: 14, body, .. } => table = ChannelTable::decode(&body),
            Message::Data { ms, blocks } => {
                for b in blocks {
                    r.push_block(ms, &b)
                }
            }
            _ => {}
        }
    }
    assert!(r.sample_counts()["mc"] > 100);
    assert_eq!(r.sample_counts()["mc"], 4 * r.sample_counts()["mv"]);
    assert!(r.stats(0, 0.0, r.duration()).unwrap().mean.is_finite());
}
#[test]
fn uart_bursts_split_across_packets_are_joined() {
    let mut r = Recording::new(&[spec(0x17, "rx", Kind::Bytes, 0)]);
    let rx = |r: &mut Recording, ms, s: &[u8]| {
        r.push_block(ms, &block(0x17, BlockBody::Bytes(s.to_vec())))
    };
    for ms in 0..=1000 {
        match ms {
            816 => rx(&mut r, ms, b"hello a"),
            817 => rx(&mut r, ms, b"pp\n"),
            818 => rx(&mut r, ms, b"next"),
            900 => rx(&mut r, ms, b"later"),
            _ => r.push_block(ms, &block(0x17, BlockBody::Bytes(Vec::new()))),
        }
    }
    let chunks = r.uart(0.0, 1.0);
    let text: Vec<_> = chunks
        .iter()
        .map(|c| (c.t, String::from_utf8_lossy(&c.bytes).into_owned()))
        .collect();
    assert_eq!(
        text,
        [
            (0.816, "hello app\n".into()),
            (0.818, "next".into()),
            (0.9, "later".into())
        ]
    );
}

#[test]
fn uart_tx_uses_newest_packet_time() {
    let mut r = Recording::new(&[spec(0x17, "rx", Kind::Bytes, 0)]);
    r.push_uart_tx(b"before");
    assert!(r.uart_channel(0x19, 0.0, 1.0).is_empty());
    r.push_block(100, &block(0x17, BlockBody::Bytes(b"rx".to_vec())));
    r.push_block(102, &block(0x17, BlockBody::Bytes(b"\n".to_vec())));
    r.push_uart_tx(b"tx\n");
    assert_eq!(r.uart_channel(0x19, 0.0, 1.0)[0].t, 0.002);
    assert_eq!(r.uart(0.0, 1.0)[0].bytes, b"rx\n");
    r.stop();
    r.push_uart_tx(b"after");
    assert_eq!(r.uart_channel(0x19, 0.0, 1.0).len(), 1);
}
