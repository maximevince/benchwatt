use benchwatt_rec::{
    Block, ChannelDesc, Data, Kind, Quantity, Recording, View, export, file::Meta,
};
use std::{fs, io::Read};
fn path(ext: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "benchwatt-test-{}-{}.{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
        ext
    ))
}
fn spec(id: u16, name: &str, unit: &str, rate_hz: u32, kind: Kind) -> ChannelDesc {
    ChannelDesc {
        id,
        name: name.into(),
        unit: unit.into(),
        rate_hz,
        kind,
        quantity: Quantity::Other,
        group: String::new(),
        bits: 0,
    }
}
fn digital(id: u16, name: &str, bits: u8) -> ChannelDesc {
    ChannelDesc {
        bits,
        ..spec(id, name, "", 1000, Kind::Digital)
    }
}
/// A 1 ms packet received at `ms`.
fn block(ms: u32, ch: u16, data: Data) -> Block {
    let len = match &data {
        Data::Analog(v) => v.len(),
        Data::Digital(v) => v.len(),
        Data::Bytes(_) => 1,
    };
    Block {
        ch,
        t: ms as f64 / 1000.0,
        dt: 0.001 / len.max(1) as f64,
        data,
    }
}
fn sample(rec: &mut Recording, ms: u32, ch: u16, values: Vec<f32>) {
    rec.push(&block(ms, ch, Data::Analog(values)))
}
#[test]
fn native_roundtrip() {
    let tx = spec(0x19, "tx", "", 0, Kind::Bytes);
    let mut rec = Recording::new(&[
        ChannelDesc {
            quantity: Quantity::Current,
            group: "dut".into(),
            ..spec(0, "mc", "A", 4000, Kind::Analog)
        },
        ChannelDesc {
            quantity: Quantity::Voltage,
            group: "dut".into(),
            ..spec(1, "mv", "V", 1000, Kind::Analog)
        },
        digital(0x16, "gpi", 2),
        spec(0x17, "rx", "", 0, Kind::Bytes),
    ]);
    for ms in [0, 1, 4, 5] {
        sample(&mut rec, ms, 0, vec![ms as f32; 4]);
        sample(&mut rec, ms, 1, vec![3.3]);
        rec.push(&block(ms, 0x16, Data::Digital(vec![ms % 2])));
        rec.push(&block(ms, 0x17, Data::Bytes(vec![ms as u8])));
    }
    rec.push_tx(&tx, b"sent\n");
    let p = path("bwrec");
    rec.save(&p, &Meta::default()).unwrap();
    let (loaded, meta) = Recording::load(&p).unwrap();
    fs::remove_file(p).unwrap();
    assert_eq!(meta.format_version, 1);
    assert_eq!(loaded.specs(), rec.specs());
    assert_eq!(loaded.sample_counts(), rec.sample_counts());
    assert_eq!(loaded.gaps(), rec.gaps());
    assert_eq!(loaded.gaps().len(), 1);
    assert_eq!(loaded.duration(), rec.duration());
    assert_eq!(loaded.edges(0.0, 1.0).len(), rec.edges(0.0, 1.0).len());
    assert_eq!(
        loaded.bytes(0x17, 0.0, 1.0).len(),
        rec.bytes(0x17, 0.0, 1.0).len()
    );
    assert_eq!(loaded.bytes(0x19, 0.0, 1.0)[0].bytes, b"sent\n");
    assert_eq!(loaded.bytes(0x19, 0.0, 1.0)[0].t, 0.005);
    for ch in [0, 1] {
        let a = rec.stats(ch, 0.0, 1.0).unwrap();
        let b = loaded.stats(ch, 0.0, 1.0).unwrap();
        assert_eq!(a.count, b.count);
        assert_eq!(a.mean, b.mean);
        match (
            rec.view(ch, 0.0, 1.0, 100).unwrap(),
            loaded.view(ch, 0.0, 1.0, 100).unwrap(),
        ) {
            (View::Raw { t: a, v: b }, View::Raw { t: c, v: d }) => {
                assert_eq!(a, c);
                assert_eq!(b.len(), d.len());
                for (x, y) in b.iter().zip(d) {
                    assert!(x == &y || (x.is_nan() && y.is_nan()));
                }
            }
            _ => panic!(),
        }
    }
}
fn close(a: f64, b: f64) {
    assert!((a - b).abs() <= 1e-12 * b.abs(), "{a} != {b}");
}
fn check_v1(r: &Recording) {
    let d = r.duration();
    close(d, 0.252);
    let mc = r.stats(0, 0.0, d).unwrap();
    assert_eq!(mc.count, 1008);
    close(mc.mean, 0.01842428294808737);
    close(mc.integral, 0.004642919302918022);
    let mv = r.stats(1, 0.0, d).unwrap();
    assert_eq!(mv.count, 251);
    close(mv.mean, 1.7971195804170403);
    let mp = r.stats(6, 0.0, d).unwrap();
    assert_eq!(mp.count, 1004);
    close(mp.integral, 0.008310769542120393);
    close(r.charge_mah(0.0, d).unwrap(), 0.0012896998063661172);
    let e = r.energy(0.0, d).unwrap();
    close(e.joules, 0.008310769542120393);
    close(e.mwh, 0.0023085470950334423);
    close(e.average_power, 0.03297924421476346);
    assert!(r.gaps().is_empty());
}
#[test]
fn loads_version_1_file() {
    let (r, meta) = Recording::load(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/v1.bwrec"
    ))
    .unwrap();
    assert_eq!(r.duration(), meta.duration_s);
    check_v1(&r);
    let specs = r.specs();
    let q: Vec<_> = specs
        .iter()
        .map(|s| (s.quantity, s.group.as_str(), s.bits))
        .collect();
    assert_eq!(
        q,
        [
            (Quantity::Current, "main", 0),
            (Quantity::Voltage, "main", 0),
            (Quantity::Power, "main", 0),
            (Quantity::Other, "", 2),
        ]
    );
    assert_eq!(r.edges(0.0, 1.0).len(), 1);
    let sr = path("sr");
    export::export_sr(&r, &sr).unwrap();
    let mut z = zip::ZipArchive::new(fs::File::open(&sr).unwrap()).unwrap();
    let mut text = String::new();
    z.by_name("metadata")
        .unwrap()
        .read_to_string(&mut text)
        .unwrap();
    fs::remove_file(sr).unwrap();
    assert!(text.contains("total probes=2\nprobe1=gpi1\nprobe2=gpi2\n"));
    let p = path("bwrec");
    r.save(&p, &meta).unwrap();
    let (again, meta2) = Recording::load(&p).unwrap();
    fs::remove_file(p).unwrap();
    assert_eq!(again.specs(), specs);
    assert_eq!(meta2.duration_s, meta.duration_s);
    check_v1(&again);
}
#[test]
fn csv_and_sr() {
    let mut rec = Recording::new(&[
        spec(0, "mc", "A", 4000, Kind::Analog),
        spec(1, "mv", "V", 1000, Kind::Analog),
        digital(0x16, "gpi", 2),
    ]);
    for ms in [0, 1, 3] {
        sample(&mut rec, ms, 0, vec![1.0; 4]);
        sample(&mut rec, ms, 1, vec![3.0]);
        rec.push(&block(ms, 0x16, Data::Digital(vec![1])));
    }
    let csv = path("csv");
    export::export_csv(&rec, &csv, None).unwrap();
    let data = fs::read_to_string(&csv).unwrap();
    assert!(data.starts_with("time_s,mc[A],mv[V]\n"));
    assert!(data.lines().any(|l| l.starts_with("0.002000000,,")));
    fs::remove_file(csv).unwrap();
    let sr = path("sr");
    export::export_sr(&rec, &sr).unwrap();
    let mut z = zip::ZipArchive::new(fs::File::open(&sr).unwrap()).unwrap();
    let mut meta = String::new();
    z.by_name("metadata")
        .unwrap()
        .read_to_string(&mut meta)
        .unwrap();
    assert!(meta.contains("analog3=mc"));
    assert!(meta.contains("total probes=2"));
    assert!(meta.contains("probe1=gpi1\nprobe2=gpi2\n"));
    assert_eq!(z.by_name("analog-1-3-1").unwrap().size(), 16 * 4);
    assert_eq!(z.by_name("logic-1-1").unwrap().size(), 16);
    fs::remove_file(sr).unwrap();
}
#[test]
fn sr_probes_follow_digital_bits() {
    for (bits, probes) in [(0, None), (3, Some(3))] {
        let mut rec = Recording::new(&[
            spec(0, "mc", "A", 4000, Kind::Analog),
            digital(0x20, "in", bits),
        ]);
        sample(&mut rec, 0, 0, vec![1.0; 4]);
        rec.push(&block(0, 0x20, Data::Digital(vec![0xff])));
        let sr = path("sr");
        export::export_sr(&rec, &sr).unwrap();
        let mut z = zip::ZipArchive::new(fs::File::open(&sr).unwrap()).unwrap();
        let mut meta = String::new();
        z.by_name("metadata")
            .unwrap()
            .read_to_string(&mut meta)
            .unwrap();
        let mut logic = Vec::new();
        if let Ok(mut f) = z.by_name("logic-1-1") {
            f.read_to_end(&mut logic).unwrap();
        }
        fs::remove_file(sr).unwrap();
        match probes {
            None => {
                assert!(!meta.contains("probe") && logic.is_empty());
                assert!(meta.contains("analog1=mc"));
            }
            Some(n) => {
                assert!(meta.contains(&format!("total probes={n}\n")));
                assert!(meta.contains("probe1=in1\nprobe2=in2\nprobe3=in3\n"));
                assert!(meta.contains("analog4=mc"));
                assert_eq!(logic, [0b111; 4]);
            }
        }
    }
}
#[test]
#[ignore = "requires sigrok-cli installed"]
fn sigrok_cli_parses() {
    let mut rec = Recording::new(&[spec(0, "mc", "A", 4000, Kind::Analog)]);
    sample(&mut rec, 0, 0, vec![1.0; 4]);
    let p = path("sr");
    export::export_sr(&rec, &p).unwrap();
    let out = std::process::Command::new("sigrok-cli")
        .arg("-i")
        .arg(&p)
        .arg("--show")
        .output()
        .unwrap();
    fs::remove_file(p).unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
