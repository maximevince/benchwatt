use benchwatt_device::{Block, Data, Device, ErrorKind, Event, Kind, Quantity, Value};
use otii_arc::{
    Arc, ArcDevice,
    adapter::convert,
    frame::{self, Decoder},
    msg::{self, BlockBody, Message},
    prop::ChannelTable,
};
use serde_json::json;
use std::{
    collections::HashMap,
    io::{Read, Write},
    os::unix::net::UnixStream,
    sync::{Arc as StdArc, Mutex, mpsc},
    time::Duration,
};

const WAIT: Duration = Duration::from_secs(2);

fn hex(s: &str) -> Vec<u8> {
    s.as_bytes()
        .chunks_exact(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}
fn block(ch: u16, body: BlockBody) -> msg::Block {
    msg::Block {
        ch,
        n: 0,
        rate_hz: 0,
        body,
    }
}
fn data(ms: u32, blocks: &[(u16, &[f32])]) -> Vec<u8> {
    let mut p = msg::DATA.to_le_bytes().to_vec();
    p.extend_from_slice(&ms.to_le_bytes());
    for (ch, v) in blocks {
        p.extend_from_slice(&ch.to_le_bytes());
        p.extend_from_slice(&(v.len() as u16).to_le_bytes());
        p.extend_from_slice(&1000u32.to_le_bytes());
        for x in *v {
            p.extend_from_slice(&x.to_le_bytes());
        }
    }
    p
}
fn monitor(ch: u32, value: f32) -> Vec<u8> {
    let mut p = msg::UPDATE.to_le_bytes().to_vec();
    p.extend_from_slice(&0u32.to_le_bytes());
    p.extend_from_slice(&2u16.to_le_bytes());
    p.extend_from_slice(&8u16.to_le_bytes());
    p.extend_from_slice(&ch.to_le_bytes());
    p.extend_from_slice(&value.to_le_bytes());
    p
}

type Log = StdArc<Mutex<Vec<(u32, Vec<u8>)>>>;

/// Answers commands like an Arc: gets from `props` (no answer for ids it does
/// not have), sets store into `props`, everything else succeeds. Before
/// acknowledging stop (0x7e) it sends one data packet with ms 1002.
struct Mock {
    out: StdArc<Mutex<UnixStream>>,
    props: StdArc<Mutex<HashMap<u32, Vec<u8>>>>,
    log: Log,
}
impl Mock {
    fn send(&self, payload: &[u8]) {
        self.out
            .lock()
            .unwrap()
            .write_all(&frame::encode(payload))
            .unwrap();
    }
    fn commands(&self) -> Vec<u32> {
        self.log.lock().unwrap().iter().map(|c| c.0).collect()
    }
}
fn mock(props: &[(u32, Vec<u8>)]) -> (ArcDevice, Mock) {
    let (client, server) = UnixStream::pair().unwrap();
    client
        .set_read_timeout(Some(Duration::from_millis(20)))
        .unwrap();
    let dev = ArcDevice::new(Arc::from_io(client.try_clone().unwrap(), client));
    let mock = Mock {
        out: StdArc::new(Mutex::new(server.try_clone().unwrap())),
        props: StdArc::new(Mutex::new(props.iter().cloned().collect())),
        log: StdArc::default(),
    };
    let (out, props, log) = (mock.out.clone(), mock.props.clone(), mock.log.clone());
    std::thread::spawn(move || {
        let mut server = server;
        let mut decoder = Decoder::new();
        let mut buf = [0; 4096];
        while let Ok(n @ 1..) = server.read(&mut buf) {
            for p in decoder.push(&buf[..n]) {
                let cmd = u32::from_le_bytes(p[4..8].try_into().unwrap());
                let args = p[8..].to_vec();
                log.lock().unwrap().push((cmd, args.clone()));
                let id = args
                    .get(..4)
                    .map(|b| u32::from_le_bytes(b.try_into().unwrap()));
                let body = match cmd {
                    0x64 => match props.lock().unwrap().get(&id.unwrap()) {
                        Some(v) => v.clone(),
                        None => continue,
                    },
                    0x66 => {
                        props
                            .lock()
                            .unwrap()
                            .insert(id.unwrap(), args[4..].to_vec());
                        Vec::new()
                    }
                    _ => Vec::new(),
                };
                let mut w = out.lock().unwrap();
                if cmd == 0x7e {
                    w.write_all(&frame::encode(&data(1002, &[(0, &[9.0])])))
                        .unwrap();
                }
                let mut r = msg::CMD_RESP.to_le_bytes().to_vec();
                r.extend_from_slice(&p[..4]);
                r.extend_from_slice(&0i32.to_le_bytes());
                r.extend_from_slice(&body);
                w.write_all(&frame::encode(&r)).unwrap();
            }
        }
    });
    (dev, mock)
}
/// Every event sent before this has been handled by the adapter.
fn sync(mock: &Mock, events: &mpsc::Receiver<Event>) {
    mock.send(&monitor(0x14, 42.0));
    loop {
        if let Event::Reading { ch: 0x14, value } = events.recv_timeout(WAIT).unwrap()
            && value == 42.0
        {
            return;
        }
    }
}

#[test]
fn convert_blocks() {
    assert_eq!(
        convert(
            1500,
            &block(0, BlockBody::Samples(vec![1.0, -2.5, 3.0, 4.0]))
        ),
        Block {
            ch: 0,
            t: 1.5,
            dt: 0.00025,
            data: Data::Analog(vec![1.0, -2.5, 3.0, 4.0]),
        }
    );
    assert_eq!(
        convert(0, &block(0x16, BlockBody::Digital(vec![1, 3]))),
        Block {
            ch: 0x16,
            t: 0.0,
            dt: 0.0005,
            data: Data::Digital(vec![1, 3]),
        }
    );
    assert_eq!(
        convert(2, &block(0x17, BlockBody::Bytes(b"abc".to_vec()))),
        Block {
            ch: 0x17,
            t: 0.002,
            dt: 0.001,
            data: Data::Bytes(b"abc".to_vec()),
        }
    );
    for body in [
        BlockBody::Samples(vec![]),
        BlockBody::Digital(vec![]),
        BlockBody::Bytes(vec![]),
    ] {
        assert_eq!(convert(7, &block(1, body)).dt, 0.001);
    }
}

#[test]
fn convert_captured_session() {
    let fixture = include_str!("../../../fixtures/session1.hex");
    let mut table = ChannelTable::default();
    let mut converted = Vec::new();
    let mut first = None;
    for line in fixture
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
    {
        match msg::parse(&hex(line), &table).unwrap() {
            Message::CmdResp {
                txn: 14,
                status: 0,
                body,
            } => table = ChannelTable::decode(&body),
            Message::Data { ms, blocks } => {
                let first = *first.get_or_insert(ms);
                converted.push(
                    blocks
                        .iter()
                        .map(|b| convert(ms.wrapping_sub(first), b))
                        .collect::<Vec<_>>(),
                );
            }
            _ => {}
        }
    }
    assert!(converted.len() > 2);
    for (i, packet) in converted.iter().take(2).enumerate() {
        let mc = packet.iter().find(|b| b.ch == 0).unwrap();
        let mv = packet.iter().find(|b| b.ch == 1).unwrap();
        assert_eq!(mc.t, i as f64 * 0.001);
        assert_eq!(mv.t, i as f64 * 0.001);
        assert_eq!(mc.dt, 0.00025);
        assert_eq!(mv.dt, 0.001);
        assert!(matches!(&mc.data, Data::Analog(v) if v.len() == 4));
        assert!(matches!(&mv.data, Data::Analog(v) if v.len() == 1));
    }
}

#[test]
fn events_are_translated() {
    let (dev, mock) = mock(&[]);
    let events = dev.events();
    assert_eq!(dev.get("4wire_active").unwrap(), Value::Null);
    let fixture = include_str!("../../../fixtures/hello_and_updates.txt");
    for line in fixture.lines().filter(|l| l.starts_with("update ")) {
        mock.send(&hex(line.rsplit_once(' ').unwrap().1));
    }
    let mut alert = msg::ALERT.to_le_bytes().to_vec();
    alert.extend_from_slice(&[0, 0, 0, 0, 6, 0, 0, 0]);
    mock.send(&alert);
    let mut property = msg::UPDATE.to_le_bytes().to_vec();
    property.extend_from_slice(&[0, 0, 0, 0, 1, 0, 8, 0, 9, 0, 0, 0, 1, 0, 0, 0]);
    mock.send(&property);
    let (mut mv, mut inputs, mut changed) = (false, false, 0);
    loop {
        match events.recv_timeout(WAIT).unwrap() {
            Event::Reading { ch: 1, value } if (value - 3.3).abs() < 0.05 => mv = true,
            Event::Inputs(_) => inputs = true,
            Event::Changed => changed += 1,
            Event::Alert { id, name } => {
                assert_eq!((id, name.as_str()), (6, "main-overcurr"));
                break;
            }
            _ => {}
        }
    }
    assert!(matches!(events.recv_timeout(WAIT).unwrap(), Event::Changed));
    assert!(mv && inputs && changed > 0);
    assert_eq!(dev.get("4wire_active").unwrap(), Value::Bool(true));

    struct FailingReader(mpsc::Receiver<()>);
    impl Read for FailingReader {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            let _ = self.0.recv();
            Err(std::io::ErrorKind::BrokenPipe.into())
        }
    }
    let (tx, rx) = mpsc::channel();
    let dev = ArcDevice::new(Arc::from_io(FailingReader(rx), std::io::sink()));
    let events = dev.events();
    tx.send(()).unwrap();
    assert!(matches!(events.recv_timeout(WAIT), Ok(Event::Disconnected)));
    let e = dev.get("main").unwrap_err();
    assert_eq!(
        (e.kind, e.message.as_str()),
        (ErrorKind::Closed, "device connection closed")
    );
}

#[test]
fn meter_streams_between_start_and_stop() {
    let (dev, mock) = mock(&[]);
    let events = dev.events();
    let meter = dev.meter().unwrap();
    let (tx, rx) = mpsc::channel();
    mock.send(&data(990, &[(0, &[7.0])]));
    sync(&mock, &events);
    meter.start(&[0, 1], tx).unwrap();
    assert_eq!(mock.commands(), [0x78, 0x78, 0x7c]);
    mock.send(&data(1000, &[(0, &[1.0, 2.0]), (1, &[3.3])]));
    mock.send(&data(1001, &[(0, &[3.0, 4.0])]));
    let got: Vec<Block> = (0..3).map(|_| rx.recv_timeout(WAIT).unwrap()).collect();
    assert_eq!(
        got,
        [
            Block {
                ch: 0,
                t: 0.0,
                dt: 0.0005,
                data: Data::Analog(vec![1.0, 2.0])
            },
            Block {
                ch: 1,
                t: 0.0,
                dt: 0.001,
                data: Data::Analog(vec![3.3])
            },
            Block {
                ch: 0,
                t: 0.001,
                dt: 0.0005,
                data: Data::Analog(vec![3.0, 4.0])
            },
        ]
    );
    meter.stop().unwrap();
    assert_eq!(&mock.commands()[3..], [0x7e, 0x78, 0x78]);
    let last = rx.try_recv().unwrap();
    assert_eq!((last.t, last.data), (0.002, Data::Analog(vec![9.0])));
    mock.send(&data(1003, &[(0, &[5.0])]));
    sync(&mock, &events);
    assert!(matches!(
        rx.try_recv(),
        Err(mpsc::TryRecvError::Disconnected)
    ));
}

#[test]
fn get_set_and_errors() {
    let (dev, mock) = mock(&[
        (1, b"Arc".to_vec()),
        (9, 1i32.to_le_bytes().to_vec()),
        (11, 3_300_000i32.to_le_bytes().to_vec()),
        (12, 3000i32.to_le_bytes().to_vec()),
        (0x29, 115200i32.to_le_bytes().to_vec()),
        (0x80b, 4_500_000i32.to_le_bytes().to_vec()),
    ]);
    let num = |key: &str| dev.get(key).unwrap().as_f64().unwrap();
    assert_eq!(dev.get("name").unwrap(), json!("Arc"));
    assert_eq!(dev.get("main").unwrap(), json!(true));
    assert_eq!(dev.get("baudrate").unwrap(), json!(115200));
    assert!((num("0xb") - 3.3).abs() < 1e-9);

    dev.set("mainvolt", json!(1.8)).unwrap();
    assert!((num("mainvolt") - 1.8).abs() < 1e-9);
    dev.set("main", json!(0)).unwrap();
    assert_eq!(dev.get("main").unwrap(), json!(false));
    dev.set("baudrate", json!(9600.7)).unwrap();
    assert_eq!(dev.get("baudrate").unwrap(), json!(9600));
    dev.set("name", json!(5)).unwrap();
    assert_eq!(dev.get("name").unwrap(), json!("5"));
    assert!(!mock.commands().contains(&0x68));
    dev.set("expvolt", json!(3.3)).unwrap();
    assert_eq!(mock.commands().last(), Some(&0x68));

    let check = |r: benchwatt_device::Result<_>, kind, message: &str| {
        let e = r.unwrap_err();
        assert_eq!((e.kind, e.message.as_str()), (kind, message));
    };
    check(
        dev.get("nope").map(drop),
        ErrorKind::Invalid,
        "unknown property nope",
    );
    check(
        dev.set("nope", json!(1)),
        ErrorKind::Invalid,
        "unknown property nope",
    );
    check(
        dev.set("main", json!("on")),
        ErrorKind::Invalid,
        "property error: wrong value type for main",
    );
    check(
        dev.set("0x99", json!(1)),
        ErrorKind::Invalid,
        "property error: unknown property 0x99",
    );
    check(
        dev.get("hwversion").map(drop),
        ErrorKind::Timeout,
        "device response timed out",
    );

    let supply = dev.supply().unwrap();
    let outputs = supply.outputs();
    assert_eq!((outputs.len(), outputs[0].n), (1, 1));
    assert!((outputs[0].voltage.max - 4.5).abs() < 1e-9);
    supply.set_enabled(1, true).unwrap();
    supply.set_voltage(1, 2.5).unwrap();
    supply.set_current_limit(1, 0.5).unwrap();
    let state = supply.state(1).unwrap();
    assert!(state.enabled);
    assert!((state.voltage - 2.5).abs() < 1e-9 && (state.current_limit - 0.5).abs() < 1e-9);
    check(supply.state(2).map(drop), ErrorKind::Invalid, "no output 2");

    let gpio = dev.gpio().unwrap();
    assert_eq!(gpio.outputs(), [None, None]);
    gpio.set_output(2, true).unwrap();
    assert_eq!(gpio.outputs(), [None, Some(true)]);
    assert_eq!(
        dev.get("gpiocfg").unwrap(),
        json!(2 << 3),
        "set_gpo writes gpiocfg"
    );
    assert!(gpio.set_output(0, true).is_err());

    let uart = dev.uart().unwrap();
    assert_eq!((uart.rx(), uart.tx()), (0x17, 0x19));
    uart.write(b"hi").unwrap();
    assert_eq!(
        mock.log.lock().unwrap().last().unwrap(),
        &(0x82, vec![0x19, 0, 0, 0, b'h', b'i'])
    );

    let info = dev.info();
    assert_eq!((info.kind.as_str(), info.model.as_str()), ("arc", ""));
    assert_eq!(
        Value::Object(info.details),
        json!({"protocol":0,"max_payload":0,"device_type":0,"features":0,"properties":{}})
    );
}

#[test]
fn channel_table() {
    let (dev, _mock) = mock(&[]);
    let channels = dev.channels();
    let ch = |name: &str| channels.iter().find(|c| c.name == name).unwrap();
    assert_eq!(channels.len(), otii_arc::prop::CHANNELS.len());
    assert_eq!(
        (ch("mc").kind, ch("mc").quantity, ch("mc").group.as_str()),
        (Kind::Analog, Quantity::Current, "main")
    );
    assert_eq!(ch("mp").group, "main");
    assert_eq!(
        (ch("ac").quantity, ch("ac").group.as_str()),
        (Quantity::Current, "")
    );
    assert_eq!((ch("gpi").kind, ch("gpi").bits), (Kind::Digital, 2));
    assert_eq!((ch("rx").kind, ch("tx").kind), (Kind::Bytes, Kind::Bytes));
    assert_eq!(ch("tp").quantity, Quantity::Temperature);
    assert_eq!(ch("mc").bits, 0);
}
