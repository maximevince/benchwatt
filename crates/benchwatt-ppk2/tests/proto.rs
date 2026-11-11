use benchwatt_device::ErrorKind;
use benchwatt_ppk2::{Calibration, SpikeFilter, convert, lost_samples, parse_calibration};

const FIXTURE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/ppk2-calibration.txt"
));

fn word(raw: u16, range: u8, counter: u8, bits: u8) -> u32 {
    u32::from(raw) | u32::from(range) << 14 | u32::from(counter) << 18 | u32::from(bits) << 24
}
fn close(a: f64, b: f64) {
    assert!((a - b).abs() <= b.abs() * 1e-12, "{a} != {b}");
}

#[test]
fn parses_the_owners_reply() {
    let c = parse_calibration(FIXTURE).unwrap();
    assert_eq!(c.r[0], 1002.6676635742188);
    assert_eq!(c.r[4], 0.056347765028476715);
    assert_eq!(c.gs[0], 1e-19, "a tiny finite value is used as sent");
    assert_eq!(c.gs[1], 73.04046630859375);
    assert_eq!(c.gi[3], 0.9688374996185303);
    assert_eq!(c.o[2], 42.90937805175781);
    assert_eq!(c.s[4], 0.0039028015453368425);
    assert_eq!(c.i[4], -0.01226027775555849);
    assert_eq!(c.ug, [1.0; 5]);
    assert_eq!(c.vdd_mv, Some(3700));
    assert_eq!(c.hw.as_deref(), Some("6407"));
    assert_eq!(c.mode, Some(2));
    assert_eq!(c.ia, Some(59.0));
    assert_eq!(c.calibrated, Some(false));
}

#[test]
fn nan_missing_and_zero_take_defaults() {
    let c =
        parse_calibration("R0: -nan\nR1: nan\nGS2: 0\nO3: -nan\nUG4: inf\nR3: 2.5\nEND\n").unwrap();
    let d = Calibration::default();
    assert_eq!(c.r, [1031.64, 101.65, 10.15, 2.5, 0.043]);
    assert_eq!(c.gs, d.gs);
    assert_eq!(c.o, [0.0; 5]);
    assert_eq!(c.ug, [1.0; 5]);
    assert_eq!((c.vdd_mv, c.hw, c.mode), (None, None, None));
}

#[test]
fn reply_without_end_is_an_error() {
    let cut = &FIXTURE[..FIXTURE.find("END").unwrap()];
    assert_eq!(parse_calibration(cut).unwrap_err().kind, ErrorKind::Invalid);
    assert!(parse_calibration("").is_err());
}

#[test]
fn converts_by_the_formula() {
    let c = parse_calibration(FIXTURE).unwrap();
    // k = 1.8 / 163840, VDD = 3700 mV.
    // Range 0, raw 1000: adc 4000, r = (4000 − 77.09832000732422)·k / 1002.6676635742188
    //   = 4.29836192232234e-5; r·(1e-19·r + 1) = 4.29836192232234e-5;
    //   S0·3.7 = 3.30060665021404e-7; + I0 (−2.0693740054867e-7) → 4.310674248769614e-5 A.
    close(
        convert(word(1000, 0, 0, 0), &c, 3700.0).amps,
        4.310674248769614e-5,
    );
    // Range 2, raw 5000: adc 20000, r = (20000 − 42.90937805175781)·k / 10.17940616607666
    //   = 0.021539090042772983; r·(12.137956619262695·r + 0.9735749959945679)
    //   = 0.02660111084602012; S2·3.7 = 3.668780582302133e-5; + I2 (7.41540861781687e-6)
    //   → 0.026645214060460956 A.
    close(
        convert(word(5000, 2, 0, 0), &c, 3700.0).amps,
        0.026645214060460956,
    );
    // Range 4, raw 12000: adc 48000, r = (48000 − 58.15695571899414)·k / 0.056347765028476715
    //   = 9.347395026857532; r·(0.055061936378479004·r + 0.9691752791404724) = 13.87023445909877;
    //   S4·3.7 = 0.014440365717746317; + I4 (−0.01226027775555849) → 13.872414547060957 A.
    close(
        convert(word(12000, 4, 0, 0), &c, 3700.0).amps,
        13.872414547060957,
    );
    // Range 1, raw 0: r = −50.37852096557617·k / 101.21644592285156 = −5.46823153820158e-6;
    //   r·(73.04046630859375·r + 0.9791056513786316) = −5.351792378495093e-6;
    //   S1·3.7 = 3.573227536435298e-6; + I1 (−2.59094713328523e-6) → −4.369511975345025e-6 A.
    close(
        convert(word(0, 1, 0, 0), &c, 3700.0).amps,
        -4.369511975345025e-6,
    );
}

#[test]
fn extracts_bit_fields() {
    let c = Calibration::default();
    let s = convert(
        0xA5 << 24 | 0x2a << 18 | 1 << 17 | 3 << 14 | 0x3fff,
        &c,
        3300.0,
    );
    assert_eq!((s.range, s.counter, s.bits), (3, 0x2a, 0xA5));
    for range in 4..8 {
        let s = convert(word(100, range, 63, 0xff), &c, 3300.0);
        assert_eq!((s.range, s.counter, s.bits), (4, 63, 0xff));
        assert_eq!(s.amps, convert(word(100, 4, 0, 0), &c, 3300.0).amps);
    }
    assert_eq!(lost_samples(5, 5), 0);
    assert_eq!(lost_samples(5, 6), 1);
    assert_eq!(lost_samples(62, 1), 3);
    assert_eq!(lost_samples(0, 63), 63);
}

#[test]
fn spike_filter_smooths_range_changes() {
    let mut f = SpikeFilter::default();
    let run = |f: &mut SpikeFilter, seq: &[(f64, u8)]| -> Vec<f64> {
        seq.iter().map(|&(x, r)| f.apply(x, r)).collect()
    };
    // Steady on range 1: passed through, both averages settle at 10.
    assert_eq!(run(&mut f, &[(10.0, 1); 3]), [10.0; 3]);
    // Switch to range 2 with a spike: three samples replaced by the 0.18 average,
    // which takes each new sample in: 10 + 0.18·90 = 26.2,
    // 26.2 + 0.18·(20 − 26.2) = 25.084, 25.084 + 0.18·(20 − 25.084) = 24.16888.
    let out = run(&mut f, &[(100.0, 2), (20.0, 2), (20.0, 2), (20.0, 2)]);
    for (a, b) in out.iter().zip([26.2, 25.084, 24.16888, 20.0]) {
        assert!((a - b).abs() < 1e-12, "{out:?}");
    }

    f.reset();
    run(&mut f, &[(1.0, 3); 3]);
    // Into range 4: the 0.06 average is used, and the first two samples are rolled
    // back out of it, so it stays at 1; the third is taken in: 1 + 0.06·(2 − 1) = 1.06.
    let out = run(&mut f, &[(50.0, 4), (2.0, 4), (2.0, 4), (2.0, 4)]);
    for (a, b) in out.iter().zip([1.0, 1.0, 1.06, 2.0]) {
        assert!((a - b).abs() < 1e-12, "{out:?}");
    }
    // The fast average was rolled back too: back on range 3 it is 1 + 0.18·(2 − 1)
    // after the third range-4 sample, then 1.18 + 0.18·(2 − 1.18) for the fourth,
    // then the new sample: 1.3276 + 0.18·(5 − 1.3276) = 1.988632.
    let out = f.apply(5.0, 3);
    assert!((out - 1.988632).abs() < 1e-12, "{out}");

    f.reset();
    assert_eq!(
        f.apply(7.0, 4),
        7.0,
        "the first sample after a reset passes through"
    );
}
