use otii_arc::{
    Arc, Error, Event,
    frame::{self, Decoder},
    msg::{self, BlockBody, Message, UpdateValue},
    prop::{self, ChannelKind, ChannelTable, PropertyValue},
};
use std::{
    io::{Read, Write},
    os::unix::net::UnixStream,
    time::Duration,
};

fn hex(s: &str) -> Vec<u8> {
    s.as_bytes()
        .chunks_exact(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}
#[test]
fn captured_session() {
    let fixture = include_str!("../../../fixtures/session1.hex");
    let mut table = ChannelTable::default();
    let mut responses = std::collections::HashMap::new();
    let mut data_ms = Vec::new();
    let mut count = 0;
    for line in fixture
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
    {
        let message = msg::parse(&hex(line), &table).unwrap();
        count += 1;
        match message {
            Message::CmdResp {
                txn,
                status: 0,
                body,
            } => {
                if txn == 14 {
                    table = ChannelTable::decode(&body);
                }
                responses.insert(txn, body);
            }
            Message::CmdResp { status: -3, .. } => {}
            Message::Data { ms, blocks } => {
                data_ms.push(ms);
                let mc = blocks.iter().find(|b| b.ch == 0).unwrap();
                let mv = blocks.iter().find(|b| b.ch == 1).unwrap();
                assert_eq!(mc.n, 4);
                assert_eq!(mv.n, 1);
                assert!(matches!(&mc.body, BlockBody::Samples(v) if v.len() == 4));
                assert!(matches!(&mv.body, BlockBody::Samples(v) if v.len() == 1));
            }
            Message::Update { records } => assert!(
                records
                    .iter()
                    .any(|r| matches!(r.value, UpdateValue::Monitor(_)))
            ),
            other => panic!("unexpected fixture message: {other:?}"),
        }
    }
    assert!(count > 80);
    let hello = &responses[&0];
    assert_eq!(u32::from_le_bytes(hello[0..4].try_into().unwrap()), 3);
    assert_eq!(u32::from_le_bytes(hello[4..8].try_into().unwrap()), 4128);
    assert_eq!(
        u32::from_le_bytes(hello[8..12].try_into().unwrap()),
        0x41726320
    );
    assert_eq!(u32::from_le_bytes(hello[12..16].try_into().unwrap()), 0x27);
    for (txn, id, expected) in [(1, 1, "Arc"), (2, 0x80, "1.3"), (3, 0x81, "3.1.3")] {
        assert_eq!(
            prop::decode(id, &responses[&txn]),
            PropertyValue::String(expected.into())
        );
    }
    for (txn, id, expected) in [(9, 11, 3.3), (10, 12, 3.0), (11, 13, 0.0), (13, 30, 1.0)] {
        assert_eq!(
            prop::decode(id, &responses[&txn]),
            PropertyValue::Number(expected)
        );
    }
    assert_eq!(
        prop::decode(13, &0x0f4240i32.to_le_bytes()),
        PropertyValue::Number(1.0)
    );
    assert_eq!(
        prop::decode(0x29, &responses[&21]),
        PropertyValue::Int(115200)
    );
    assert_eq!(table.get(0).rate_hz, 4000);
    assert_eq!(table.get(0).flags & 0x80, 0x80);
    assert_eq!(table.get(1).rate_hz, 1000);
    assert_eq!(table.get(0x17).kind, ChannelKind::Bytes);
    assert_eq!(table.get(0x16).kind, ChannelKind::Digital);
    assert!(!data_ms.is_empty());
    assert!(data_ms.windows(2).all(|w| w[1] == w[0] + 1));
}
#[test]
fn captured_updates() {
    let fixture = include_str!("../../../fixtures/hello_and_updates.txt");
    let mut mv = false;
    let mut gpi = false;
    let mut four_wire = false;
    for line in fixture
        .lines()
        .filter(|l| l.starts_with("cmd ") || l.starts_with("update "))
    {
        let (_, rest) = line.split_once(' ').unwrap();
        let (size, payload) = rest.split_once(' ').unwrap();
        let bytes = hex(payload);
        assert_eq!(bytes.len(), size.parse::<usize>().unwrap());
        if let Message::Update { records } = msg::parse(&bytes, &ChannelTable::default()).unwrap() {
            for r in records {
                match (r.id, r.value) {
                    (1, UpdateValue::Monitor(v)) if (v - 3.3).abs() < 0.05 => mv = true,
                    (0x16, UpdateValue::Gpi(_)) => gpi = true,
                    (0x10002, UpdateValue::FourWire(_)) => four_wire = true,
                    _ => {}
                }
            }
        }
    }
    assert!(mv && gpi && four_wire);
}
#[test]
fn framing_and_set_encoding() {
    for n in [0, 1, 7, 4128] {
        let payload = vec![0xa5; n];
        let frame = frame::encode(&payload);
        let mut d = Decoder::new();
        assert!(d.push(&[0, 1, 2]).is_empty());
        for byte in frame {
            let result = d.push(&[byte]);
            if !result.is_empty() {
                assert_eq!(result, vec![payload.clone()]);
            }
        }
        let mut bad = frame::encode(&payload);
        bad[6] ^= 1;
        assert!(d.push(&bad).is_empty());
        assert_eq!(d.bad_checksum_count(), 1);
    }
    assert_eq!(
        prop::encode_set(11, &PropertyValue::Number(1.8)).unwrap(),
        hex("0b00000040771b00")
    );
    assert_eq!(
        prop::encode_set(0x32, &PropertyValue::Int(2)).unwrap(),
        hex("3200000002000000")
    );
    for id in (0x51..=0x60).chain([0xb1]).chain(0xb5..=0xbb) {
        assert!(prop::encode_set(id, &PropertyValue::Int(1)).is_err());
    }
}
#[test]
fn out_of_order_responses_and_timeout() {
    let (client, mut server) = UnixStream::pair().unwrap();
    client
        .set_read_timeout(Some(Duration::from_millis(20)))
        .unwrap();
    server
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let arc = Arc::from_io(client.try_clone().unwrap(), client);
    std::thread::scope(|scope| {
        let first = scope.spawn(|| arc.get(1));
        let second = scope.spawn(|| arc.get(0x80));
        let mut decoder = Decoder::new();
        let mut commands = Vec::new();
        let mut buf = [0; 256];
        while commands.len() < 2 {
            let n = server.read(&mut buf).unwrap();
            commands.extend(decoder.push(&buf[..n]));
        }
        for payload in commands.into_iter().rev() {
            assert_eq!(u32::from_le_bytes(payload[4..8].try_into().unwrap()), 0x64);
            assert_eq!(&payload[12..16], &[0, 0, 0, 0]);
            let txn = &payload[..4];
            let mut response = msg::CMD_RESP.to_le_bytes().to_vec();
            response.extend_from_slice(txn);
            response.extend_from_slice(&0i32.to_le_bytes());
            response.extend_from_slice(&payload[8..12]);
            server.write_all(&frame::encode(&response)).unwrap();
        }
        assert_eq!(first.join().unwrap().unwrap(), 1u32.to_le_bytes());
        assert_eq!(second.join().unwrap().unwrap(), 0x80u32.to_le_bytes());
    });
    assert!(matches!(arc.get(0x81), Err(Error::Timeout)));
}

#[test]
fn reader_failure_closes_session_once() {
    struct FailingReader(std::sync::mpsc::Receiver<()>);
    impl Read for FailingReader {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            let _ = self.0.recv();
            Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "unplugged",
            ))
        }
    }
    let (tx, rx) = std::sync::mpsc::channel();
    let arc = Arc::from_io(FailingReader(rx), std::io::sink());
    let events = arc.events();
    tx.send(()).unwrap();
    assert!(matches!(
        events.recv_timeout(Duration::from_secs(1)),
        Ok(Event::Disconnected)
    ));
    assert!(matches!(arc.get(9), Err(Error::Closed)));
    assert!(matches!(
        events.recv_timeout(Duration::from_millis(20)),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout)
    ));
    assert_eq!(
        serde_json::to_string(&Error::Closed).unwrap(),
        "\"device connection closed\""
    );
}
