use benchwatt_rec::{ChannelSpec, Kind, Recording, View, export, file::Meta};
use otii_arc::msg::{Block, BlockBody};
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
fn spec(id: u16, name: &str, unit: &str, rate_hz: u32, kind: Kind) -> ChannelSpec {
    ChannelSpec {
        id,
        name: name.into(),
        unit: unit.into(),
        rate_hz,
        kind,
    }
}
fn block(ch: u16, body: BlockBody) -> Block {
    Block {
        ch,
        n: 1,
        rate_hz: 1000,
        body,
    }
}
fn sample(rec: &mut Recording, ms: u32, ch: u16, values: Vec<f64>) {
    rec.push_block(ms, &block(ch, BlockBody::Samples(values)))
}
#[test]
fn native_roundtrip() {
    let mut rec = Recording::new(&[
        spec(0, "mc", "A", 4000, Kind::Analog),
        spec(1, "mv", "V", 1000, Kind::Analog),
        spec(0x16, "gpi", "", 1000, Kind::Digital),
        spec(0x17, "rx", "", 0, Kind::Bytes),
    ]);
    for ms in [0, 1, 4, 5] {
        sample(&mut rec, ms, 0, vec![ms as f64; 4]);
        sample(&mut rec, ms, 1, vec![3.3]);
        rec.push_block(ms, &block(0x16, BlockBody::Digital(vec![ms % 2])));
        rec.push_block(ms, &block(0x17, BlockBody::Bytes(vec![ms as u8])));
    }
    rec.push_uart_tx(b"sent\n");
    let p = path("bwrec");
    rec.save(&p, &Meta::default()).unwrap();
    let (loaded, meta) = Recording::load(&p).unwrap();
    fs::remove_file(p).unwrap();
    assert_eq!(meta.format_version, 1);
    assert_eq!(loaded.sample_counts(), rec.sample_counts());
    assert_eq!(loaded.gaps(), rec.gaps());
    assert_eq!(loaded.edges(0.0, 1.0).len(), rec.edges(0.0, 1.0).len());
    assert_eq!(loaded.uart(0.0, 1.0).len(), rec.uart(0.0, 1.0).len());
    assert_eq!(loaded.uart_channel(0x19, 0.0, 1.0)[0].bytes, b"sent\n");
    assert_eq!(loaded.uart_channel(0x19, 0.0, 1.0)[0].t, 0.005);
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
#[test]
fn csv_and_sr() {
    let mut rec = Recording::new(&[
        spec(0, "mc", "A", 4000, Kind::Analog),
        spec(1, "mv", "V", 1000, Kind::Analog),
        spec(0x16, "gpi", "", 1000, Kind::Digital),
    ]);
    for ms in [0, 1, 3] {
        sample(&mut rec, ms, 0, vec![1.0; 4]);
        sample(&mut rec, ms, 1, vec![3.0]);
        rec.push_block(ms, &block(0x16, BlockBody::Digital(vec![1])));
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
    assert_eq!(z.by_name("analog-1-3-1").unwrap().size(), 16 * 4);
    assert_eq!(z.by_name("logic-1-1").unwrap().size(), 16);
    fs::remove_file(sr).unwrap();
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
