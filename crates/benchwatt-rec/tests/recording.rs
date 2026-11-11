use benchwatt_rec::{Block, ChannelDesc, Data, Gap, Kind, Quantity, Recording, View};
fn spec(id: u16, name: &str, kind: Kind, rate: u32) -> ChannelDesc {
    ChannelDesc {
        label: String::new(),
        extra: false,
        set: false,
        id,
        name: name.into(),
        unit: String::new(),
        rate_hz: rate,
        kind,
        quantity: Quantity::Other,
        group: String::new(),
        bits: 0,
    }
}
fn measured(id: u16, name: &str, rate: u32, quantity: Quantity, group: &str) -> ChannelDesc {
    ChannelDesc {
        label: String::new(),
        extra: false,
        set: false,
        quantity,
        group: group.into(),
        ..spec(id, name, Kind::Analog, rate)
    }
}
/// A 1 ms packet received at `ms`, on a time axis starting at `first` ms.
fn block(first: u32, ms: u32, ch: u16, data: Data) -> Block {
    let len = match &data {
        Data::Analog(v) => v.len(),
        Data::Digital(v) => v.len(),
        Data::Bytes(_) => 1,
    };
    Block {
        ch,
        t: (ms - first) as f64 / 1000.0,
        dt: 0.001 / len.max(1) as f64,
        data,
    }
}
fn feed(r: &mut Recording, ms: u32, ch: u16, v: Vec<f32>) {
    r.push(&block(0, ms, ch, Data::Analog(v)))
}
#[test]
fn charge_and_energy() {
    let mut r = Recording::new(&[
        measured(0, "mc", 4000, Quantity::Current, "main"),
        measured(1, "mv", 1000, Quantity::Voltage, "main"),
        measured(6, "mp", 4000, Quantity::Power, "main"),
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
fn primary_group_ignores_ungrouped_channels() {
    let mut r = Recording::new(&[
        measured(0, "stray", 1000, Quantity::Current, ""),
        measured(3, "mc", 1000, Quantity::Current, "dut"),
        measured(4, "mv", 1000, Quantity::Voltage, "dut"),
        measured(5, "other", 1000, Quantity::Current, "aux"),
    ]);
    for ms in 0..1000 {
        feed(&mut r, ms, 0, vec![5.0]);
        feed(&mut r, ms, 3, vec![0.5]);
        feed(&mut r, ms, 4, vec![2.0]);
        feed(&mut r, ms, 5, vec![7.0]);
    }
    assert!((r.charge_mah(0.0, 1.0).unwrap() - 0.5 / 3.6).abs() < 1e-9);
    assert!((r.energy(0.0, 1.0).unwrap().joules - 1.0).abs() < 1e-9);
    let ungrouped = Recording::new(&[measured(0, "mc", 1000, Quantity::Current, "")]);
    assert!(ungrouped.charge_mah(0.0, 1.0).is_none());
    assert!(ungrouped.energy(0.0, 1.0).is_none());
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
        feed(&mut r, ms, 0, v.iter().map(|&x| x as f32).collect());
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
    r.push(&block(100, 100, 0, Data::Analog(vec![1.0; 4])));
    r.push(&block(100, 100, 0x16, Data::Digital(vec![0, 1, 1])));
    r.push(&block(100, 110, 0, Data::Analog(vec![2.0; 4])));
    r.push(&block(100, 110, 0x16, Data::Digital(vec![1, 0])));
    r.push(&block(100, 110, 0x17, Data::Bytes(b"tick\n".to_vec())));
    assert_eq!(r.gaps().len(), 1);
    assert!(matches!(r.view(0, 0.003, 0.008, 10), Some(View::Raw { t, .. }) if t.is_empty()));
    assert!(
        matches!(r.view(0, 0.0, 0.02, 10), Some(View::Raw { v, .. }) if v.iter().any(|x| x.is_nan()))
    );
    assert_eq!(r.edges(0.0, 1.0).len(), 3);
    assert_eq!(r.bytes(0x17, 0.0, 1.0)[0].bytes, b"tick\n");
    assert_eq!(r.sample_counts()["mc"], 8);
}
#[test]
fn edges_come_from_the_lowest_digital_channel() {
    let mut r = Recording::new(&[
        spec(9, "late", Kind::Digital, 1000),
        spec(4, "early", Kind::Digital, 1000),
    ]);
    r.push(&block(0, 0, 9, Data::Digital(vec![0, 1, 0])));
    r.push(&block(0, 0, 4, Data::Digital(vec![3])));
    assert_eq!(r.edges(0.0, 1.0).len(), 1);
    assert_eq!(r.edges(0.0, 1.0)[0].mask, 3);
}
#[test]
fn gaps_are_detected_per_channel_and_merged() {
    let mut r = Recording::new(&[
        spec(0, "mc", Kind::Analog, 4000),
        spec(1, "mv", Kind::Analog, 1000),
        spec(0x17, "rx", Kind::Bytes, 0),
    ]);
    for ms in (0..20000).filter(|ms| !(5000..5007).contains(ms)) {
        let t = ms as f64 / 1000.0;
        r.push(&Block {
            ch: 0,
            t,
            dt: 0.001 / 4.0,
            data: Data::Analog(vec![1.0; 4]),
        });
        r.push(&Block {
            ch: 1,
            t,
            dt: 0.001,
            data: Data::Analog(vec![3.3]),
        });
        if ms % 50 == 0 {
            r.push(&Block {
                ch: 0x17,
                t,
                dt: 0.001,
                data: Data::Bytes(b"x".to_vec()),
            });
        }
    }
    let gaps = r.gaps();
    assert_eq!(gaps.len(), 1, "{gaps:?}");
    assert!((gaps[0].from - 5.0).abs() < 1e-12 && (gaps[0].to - 5.007).abs() < 1e-12);
    // A gap seen by only one channel still counts.
    r.push(&block(0, 20005, 1, Data::Analog(vec![3.3])));
    assert_eq!(r.gaps().len(), 2);
    assert_eq!(
        r.gaps()[1],
        Gap {
            from: 20.0,
            to: 20.005
        }
    );
}
#[test]
fn duration_is_the_latest_block_end() {
    let mut r = Recording::new(&[
        spec(0, "mc", Kind::Analog, 4000),
        spec(0x17, "rx", Kind::Bytes, 0),
    ]);
    assert_eq!(r.duration(), 0.0);
    r.push(&block(0, 7, 0x30, Data::Analog(vec![1.0])));
    r.push(&block(0, 9, 0, Data::Digital(vec![1])));
    r.push(&block(0, 9, 0, Data::Analog(vec![])));
    assert_eq!(r.duration(), 0.0);
    for ms in 0..250 {
        feed(&mut r, ms, 0, vec![1.0; 4]);
    }
    assert!((r.duration() - 0.25).abs() < 1e-12);
    r.push(&block(0, 300, 0x17, Data::Bytes(vec![])));
    assert!((r.duration() - 0.301).abs() < 1e-12);
    r.stop();
    feed(&mut r, 400, 0, vec![1.0; 4]);
    assert!((r.duration() - 0.301).abs() < 1e-12);
}
#[test]
fn step_extrema_and_fallback_energy() {
    let mut r = Recording::new(&[
        measured(0, "mc", 4000, Quantity::Current, "main"),
        measured(1, "mv", 1000, Quantity::Voltage, "main"),
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
        feed(&mut r, i * 3, 0, vec![i as f32; 4]);
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
#[test]
fn uart_bursts_split_across_packets_are_joined() {
    let mut r = Recording::new(&[spec(0x17, "rx", Kind::Bytes, 0)]);
    let rx = |r: &mut Recording, ms, s: &[u8]| r.push(&block(0, ms, 0x17, Data::Bytes(s.to_vec())));
    for ms in 0..=1000 {
        match ms {
            816 => rx(&mut r, ms, b"hello a"),
            817 => rx(&mut r, ms, b"pp\n"),
            818 => rx(&mut r, ms, b"next"),
            900 => rx(&mut r, ms, b"later"),
            _ => rx(&mut r, ms, b""),
        }
    }
    let chunks = r.bytes(0x17, 0.0, 1.0);
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
fn tx_uses_newest_packet_time() {
    let tx = spec(0x19, "tx", Kind::Bytes, 0);
    let mut r = Recording::new(&[spec(0x17, "rx", Kind::Bytes, 0)]);
    r.push_tx(&tx, b"before");
    assert!(r.bytes(0x19, 0.0, 1.0).is_empty());
    assert!(!r.specs().iter().any(|s| s.id == 0x19));
    r.push(&block(100, 100, 0x17, Data::Bytes(b"rx".to_vec())));
    r.push(&block(100, 102, 0x17, Data::Bytes(b"\n".to_vec())));
    r.push_tx(&tx, b"");
    assert!(r.bytes(0x19, 0.0, 1.0).is_empty());
    r.push_tx(&tx, b"tx\n");
    assert_eq!(r.bytes(0x19, 0.0, 1.0)[0].t, 0.002);
    assert_eq!(r.specs().iter().find(|s| s.id == 0x19), Some(&tx));
    assert_eq!(r.bytes(0x17, 0.0, 1.0)[0].bytes, b"rx\n");
    r.stop();
    r.push_tx(&tx, b"after");
    assert_eq!(r.bytes(0x19, 0.0, 1.0).len(), 1);
}
/// Statistics over a long range use stored sums; they must agree with adding
/// up the samples, whatever the range, packet sizes and gaps.
#[test]
fn stats_over_long_ranges_match_the_samples() {
    let mut r = Recording::new(&[spec(0, "x", Kind::Analog, 100_000)]);
    let mut samples = Vec::new();
    let mut t = 0.0;
    let mut seed = 1u32;
    for packet in 0..300 {
        let (len, dt) = if packet % 3 == 0 {
            (70, 1e-5)
        } else {
            (33, 2e-5)
        };
        if packet == 120 {
            t += 0.5;
        }
        let data: Vec<f32> = (0..len)
            .map(|_| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (seed >> 8) as f32 / 16_777_216.0 - 0.5
            })
            .collect();
        for (k, &v) in data.iter().enumerate() {
            samples.push((t + k as f64 * dt, dt, v as f64));
        }
        r.push(&Block {
            ch: 0,
            t,
            dt,
            data: Data::Analog(data),
        });
        t += len as f64 * dt;
    }
    for (t0, t1) in [
        (0.0, t),
        (0.0123, 0.0871),
        (0.03, 0.7),
        (0.2, 0.25),
        (0.6, t),
    ] {
        let inside: Vec<_> = samples
            .iter()
            .filter(|(at, _, _)| *at >= t0 && *at <= t1)
            .collect();
        let Some(s) = r.stats(0, t0, t1) else {
            assert!(inside.is_empty(), "{t0}..{t1}");
            continue;
        };
        let n = inside.len() as f64;
        let sum: f64 = inside.iter().map(|x| x.2).sum();
        let sq: f64 = inside.iter().map(|x| x.2 * x.2).sum();
        let integral: f64 = inside.iter().map(|x| x.2 * x.1).sum();
        let min = inside.iter().map(|x| x.2).fold(f64::INFINITY, f64::min);
        let max = inside.iter().map(|x| x.2).fold(f64::NEG_INFINITY, f64::max);
        assert_eq!(s.count, inside.len(), "{t0}..{t1}");
        assert_eq!((s.min, s.max), (min, max), "{t0}..{t1}");
        assert!((s.mean - sum / n).abs() < 1e-12, "{t0}..{t1}");
        assert!((s.rms - (sq / n).sqrt()).abs() < 1e-12, "{t0}..{t1}");
        assert!((s.integral - integral).abs() < 1e-12, "{t0}..{t1}");
    }
}
/// Without a power channel, energy is current times the voltage at that
/// moment; a voltage sampled far more slowly must give the same answer.
#[test]
fn energy_from_a_slow_voltage() {
    let mut r = Recording::new(&[
        measured(0, "i", 10_000, Quantity::Current, "out"),
        measured(1, "v", 100, Quantity::Voltage, "out"),
    ]);
    let mut current = Vec::new();
    for packet in 0..100 {
        let t = packet as f64 * 0.01;
        let data: Vec<f32> = (0..100)
            .map(|k| 0.01 + (packet * 100 + k) as f32 * 1e-6)
            .collect();
        for (k, &v) in data.iter().enumerate() {
            current.push((t + k as f64 * 1e-4, v as f64));
        }
        r.push(&Block {
            ch: 0,
            t,
            dt: 1e-4,
            data: Data::Analog(data),
        });
    }
    // One voltage sample per 10 ms, steady for a while, then stepping, with
    // nothing between 0.50 and 0.60 s.
    let mut voltage = Vec::new();
    for n in (0..50).chain(60..100) {
        let v = if n < 30 {
            1.8f32
        } else {
            1.8 + n as f32 * 0.01
        };
        voltage.push((n as f64 * 0.01, v as f64));
        r.push(&Block {
            ch: 1,
            t: n as f64 * 0.01,
            dt: 0.01,
            data: Data::Analog(vec![v]),
        });
    }
    // Ranges end between samples, so rounding cannot move a sample across an end.
    for (t0, t1) in [
        (0.0, 1.0),
        (0.12345, 0.77777),
        (0.45005, 0.65005),
        (0.52005, 0.58005),
    ] {
        let mut joules = 0.0;
        let mut any = false;
        for &(t, i) in current.iter().filter(|(t, _)| *t >= t0 && *t <= t1) {
            if let Some(&(_, v)) = voltage.iter().rfind(|(at, _)| *at <= t && t < *at + 0.01) {
                joules += i * v * 1e-4;
                any = true;
            }
        }
        match r.energy(t0, t1) {
            Some(e) => assert!(
                (e.joules - joules).abs() < 1e-9,
                "{t0}..{t1}: {} vs {joules}",
                e.joules
            ),
            None => assert!(!any, "{t0}..{t1}"),
        }
    }
}
