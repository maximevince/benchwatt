use benchwatt_device::{Block, Data, Device, ErrorKind, Event, Range, Value};
use benchwatt_gpp::Gpp;
use std::{
    collections::{HashMap, VecDeque},
    io::{Read, Write},
    net::Shutdown,
    os::unix::net::UnixStream,
    sync::{Arc, Mutex, mpsc},
    thread,
    time::{Duration, Instant},
};

const WAIT: Duration = Duration::from_secs(2);
const IDN_4323: &str = "GW INSTEK,GPP-4323,SN:GEV810395,V1.16";

/// Answers like the owner's GPP-4323 (replies captured from it): a query of a
/// known header returns its value, a set of a known header stores the text,
/// anything else gets no reply and queues `Undefined Header`. A value starting
/// with `late:` is answered after 700 ms.
struct Mock {
    raw: Arc<Mutex<Vec<u8>>>,
    values: Arc<Mutex<HashMap<String, String>>>,
    errors: Arc<Mutex<VecDeque<String>>>,
    server: UnixStream,
}
impl Mock {
    fn lines(&self) -> Vec<String> {
        String::from_utf8(self.raw.lock().unwrap().clone())
            .unwrap()
            .lines()
            .map(String::from)
            .collect()
    }
    /// What was written, without the polling.
    fn commands(&self) -> Vec<String> {
        self.lines()
            .into_iter()
            .filter(|l| !l.starts_with(":MEASure"))
            .collect()
    }
    fn text(&self) -> String {
        String::from_utf8(self.raw.lock().unwrap().clone()).unwrap()
    }
    fn set(&self, key: &str, value: &str) {
        self.values.lock().unwrap().insert(key.into(), value.into());
    }
}
fn values(idn: &str) -> HashMap<String, String> {
    let mut v: HashMap<String, String> = [
        ("*IDN", idn),
        (":MEASure:VOLTage:ALL", "5.0012,0.0000,0.0000,0.0000"),
        (":MEASure:CURRent:ALL", "0.0003,0.0000,0.0000,0.0000"),
    ]
    .into_iter()
    .map(|(k, v)| (k.into(), v.into()))
    .collect();
    let set = ["5.000", "8.000", "5.000", "03.000"];
    let limit = ["0.5000", "1.0000", "0.1000", "0.5000"];
    for n in 1..=4 {
        v.insert(format!(":SOURce{n}:VOLTage"), set[n - 1].into());
        v.insert(format!(":SOURce{n}:CURRent"), limit[n - 1].into());
        v.insert(
            format!(":OUTPut{n}:STATe"),
            if n == 1 { "ON" } else { "OFF" }.into(),
        );
        v.insert(format!(":OUTPut{n}:OVP"), "35.0".into());
        v.insert(format!(":OUTPut{n}:OCP"), "3.50".into());
        v.insert(format!(":OUTPut{n}:OVP:STATe"), "OFF".into());
        v.insert(format!(":OUTPut{n}:OCP:STATe"), "OFF".into());
    }
    v
}
fn serve(values: HashMap<String, String>) -> (UnixStream, Mock) {
    let (client, server) = UnixStream::pair().unwrap();
    client
        .set_read_timeout(Some(Duration::from_millis(20)))
        .unwrap();
    let mock = Mock {
        raw: Arc::default(),
        values: Arc::new(Mutex::new(values)),
        errors: Arc::default(),
        server: server.try_clone().unwrap(),
    };
    let (raw, values, errors) = (mock.raw.clone(), mock.values.clone(), mock.errors.clone());
    let out = Arc::new(Mutex::new(server.try_clone().unwrap()));
    thread::spawn(move || {
        let mut server = server;
        let (mut buf, mut line) = ([0u8; 256], Vec::new());
        while let Ok(n @ 1..) = server.read(&mut buf) {
            raw.lock().unwrap().extend_from_slice(&buf[..n]);
            for &b in &buf[..n] {
                if b != b'\n' {
                    line.push(b);
                    continue;
                }
                let text = String::from_utf8(std::mem::take(&mut line)).unwrap();
                let reply = |s: String| {
                    let _ = out.lock().unwrap().write_all(format!("{s}\n").as_bytes());
                };
                let mut values = values.lock().unwrap();
                if text == ":SYSTem:ERRor?" {
                    let e = errors.lock().unwrap().pop_front();
                    reply(e.unwrap_or_else(|| "No Error".into()));
                } else if let Some(header) = text.strip_suffix('?') {
                    match values.get(header) {
                        Some(v) if v.starts_with("late:") => {
                            let (out, v) = (out.clone(), v[5..].to_string());
                            thread::spawn(move || {
                                thread::sleep(Duration::from_millis(700));
                                let _ = out.lock().unwrap().write_all(format!("{v}\n").as_bytes());
                            });
                        }
                        Some(v) => reply(v.clone()),
                        None => errors.lock().unwrap().push_back("Undefined Header".into()),
                    }
                } else if let Some((header, v)) = text.split_once(' ')
                    && values.contains_key(header)
                {
                    values.insert(header.into(), v.into());
                } else if text != "LOCAL" {
                    errors.lock().unwrap().push_back("Undefined Header".into());
                }
            }
        }
    });
    (client, mock)
}
fn open_with(values: HashMap<String, String>) -> (Gpp, Mock) {
    let (client, mock) = serve(values);
    let gpp = Gpp::with_io(client.try_clone().unwrap(), client).unwrap();
    (gpp, mock)
}
fn open(idn: &str) -> (Gpp, Mock) {
    open_with(values(idn))
}
fn id(gpp: &Gpp, name: &str) -> u16 {
    gpp.channels().iter().find(|c| c.name == name).unwrap().id
}
fn range(min: f64, max: f64) -> Range {
    Range { min, max }
}

#[test]
fn identity_variants() {
    for (idn, serial) in [
        (IDN_4323, "GEV810395"),
        ("GW INSTEK, GPP-4323, SN:GEV810395, V1.16", "GEV810395"),
        ("GW INSTEK,GPP-4323,GEV810395,V1.16", "GEV810395"),
    ] {
        let (gpp, _) = open(idn);
        let info = gpp.info();
        assert_eq!(
            (
                info.kind.as_str(),
                info.model.as_str(),
                info.serial.as_str()
            ),
            ("gpp", "GPP-4323", serial)
        );
        assert_eq!(info.firmware, "V1.16");
        assert_eq!(info.details["identity"], Value::from(idn));
    }
    for idn in [
        "RIGOL TECHNOLOGIES,DP832,DP8A1,00.01",
        "GW INSTEK,GPD-4303S,X,V1.00",
    ] {
        let (client, mock) = serve(values(idn));
        let err = Gpp::with_io(client.try_clone().unwrap(), client)
            .err()
            .unwrap();
        assert_eq!(err.kind, ErrorKind::Invalid, "{idn}");
        assert_eq!(mock.lines(), ["*IDN?"], "nothing more is sent to it");
    }
}

#[test]
fn model_table() {
    let outputs = |idn: &str| {
        let (gpp, _) = open(idn);
        let s = gpp.supply().unwrap();
        s.outputs()
            .into_iter()
            .map(|o| (o.label, o.voltage, o.voltage_steps, o.current_limit))
            .collect::<Vec<_>>()
    };
    let main = |n: u8| {
        (
            format!("CH{n}"),
            range(0.0, 32.0),
            vec![],
            Some(range(0.0, 3.0)),
        )
    };
    assert_eq!(
        outputs("GW INSTEK,GPP-1326,SN:xxxxxxxx,V1.00"),
        [(
            "CH1".into(),
            range(0.0, 32.0),
            vec![],
            Some(range(0.0, 6.0))
        )]
    );
    assert_eq!(
        outputs("GW INSTEK,GPP-2323,SN:xxxxxxxx,V1.00"),
        [main(1), main(2)]
    );
    assert_eq!(
        outputs("GW INSTEK,GPP-3323,SN:xxxxxxxx,V1.00"),
        [
            main(1),
            main(2),
            (
                "CH3".into(),
                range(1.8, 5.0),
                vec![1.8, 2.5, 3.3, 5.0],
                None
            )
        ]
    );
    assert_eq!(
        outputs(IDN_4323),
        [
            main(1),
            main(2),
            ("CH3".into(), range(0.0, 5.5), vec![], Some(range(0.0, 1.1))),
            (
                "CH4".into(),
                range(0.0, 16.0),
                vec![],
                Some(range(0.0, 1.1))
            ),
        ]
    );
    assert_eq!(outputs("GW INSTEK,GPP-9999,SN:x,V9.99"), [main(1), main(2)]);

    let (gpp, _) = open("GW INSTEK,GPP-3323,SN:xxxxxxxx,V1.00");
    let channels = gpp.channels();
    let names: Vec<&str> = channels.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "ch1.v", "ch1.i", "ch1.p", "ch2.v", "ch2.i", "ch2.p", "ch3.v"
        ]
    );
    let ch3 = channels.last().unwrap();
    assert_eq!(
        (ch3.label.as_str(), ch3.unit.as_str(), ch3.group.as_str()),
        ("CH3 voltage", "V", "ch3")
    );
    assert!(ch3.set);
    assert!(channels[..6].iter().all(|c| !c.set && c.rate_hz == 10));
    let mut ids: Vec<u16> = channels.iter().map(|c| c.id).collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), 7);
    let keys: Vec<String> = gpp.settings().into_iter().map(|s| s.key).collect();
    assert!(keys.contains(&"ovp2_on".into()) && !keys.iter().any(|k| k.contains('3')));
}

#[test]
fn set_commands() {
    let (gpp, mock) = open(IDN_4323);
    let events = gpp.events();
    let s = gpp.supply().unwrap();
    s.set_voltage(1, 5.0).unwrap();
    s.set_current_limit(2, 0.25).unwrap();
    s.set_enabled(3, true).unwrap();
    s.set_enabled(3, false).unwrap();
    s.set_voltage(4, 15.5).unwrap();
    let text = mock.text();
    for sent in [
        ":SOURce1:VOLTage 5.000\n:SYSTem:ERRor?\n",
        ":SOURce2:CURRent 0.2500\n:SYSTem:ERRor?\n",
        ":OUTPut3:STATe ON\n:SYSTem:ERRor?\n",
        ":OUTPut3:STATe OFF\n:SYSTem:ERRor?\n",
        ":SOURce4:VOLTage 15.500\n:SYSTem:ERRor?\n",
    ] {
        assert!(text.contains(sent), "{sent:?} in {text:?}");
    }
    assert!(!text.contains("\n\n") && !text.contains('\r') && !text.contains("FAST"));
    assert!(!text.contains("STATUS") && !text.contains("TRACK"));
    let changed = events
        .try_iter()
        .filter(|e| matches!(e, Event::Changed))
        .count();
    assert_eq!(changed, 5);
}

#[test]
fn out_of_range_is_refused_before_sending() {
    let (gpp, mock) = open("GW INSTEK,GPP-3323,SN:xxxxxxxx,V1.00");
    let before = mock.commands();
    let s = gpp.supply().unwrap();
    for e in [
        s.set_voltage(1, 32.5),
        s.set_voltage(1, -0.1),
        s.set_voltage(1, f64::NAN),
        s.set_current_limit(2, 3.01),
        s.set_voltage(3, 3.0),
        s.set_current_limit(3, 1.0),
        s.set_voltage(4, 1.0),
        s.set_enabled(0, true),
        gpp.set("ovp1", 36.0.into()),
        gpp.set("ocp1", 0.0.into()),
        gpp.set("ovp1_on", "yes".into()),
        gpp.set("poll_rate", 51.into()),
        gpp.set("ovp3", 5.0.into()),
        gpp.set("nope", 1.into()),
    ] {
        assert_eq!(e.unwrap_err().kind, ErrorKind::Invalid);
    }
    assert_eq!(mock.commands(), before);
    s.set_voltage(3, 3.3).unwrap();
    assert!(mock.text().contains(":SOURce3:VOLTage 3.300\n"));
}

#[test]
fn device_errors_fail_the_set() {
    let (gpp, mock) = open(IDN_4323);
    let events = gpp.events();
    mock.errors
        .lock()
        .unwrap()
        .push_back("Data out of range".into());
    let e = gpp.supply().unwrap().set_voltage(1, 31.0).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Invalid);
    assert!(e.message.contains("Data out of range"), "{}", e.message);
    assert!(events.try_iter().all(|e| !matches!(e, Event::Changed)));
    gpp.supply().unwrap().set_voltage(1, 31.0).unwrap();
}

#[test]
fn state() {
    let (gpp, mock) = open(IDN_4323);
    let s = gpp.supply().unwrap();
    let st = s.state(1).unwrap();
    assert_eq!((st.enabled, st.voltage, st.current_limit), (true, 5.0, 0.5));
    let st = s.state(4).unwrap();
    assert_eq!(
        (st.enabled, st.voltage, st.current_limit),
        (false, 3.0, 0.5)
    );
    s.set_voltage(2, 12.0).unwrap();
    assert_eq!(s.state(2).unwrap().voltage, 12.0);
    assert!(s.state(5).is_err());
    let cmds = mock.commands();
    assert!(cmds.contains(&":SOURce1:VOLTage?".into()));
    assert!(cmds.contains(&":SOURce1:CURRent?".into()));
    assert!(cmds.contains(&":OUTPut1:STATe?".into()));
}

#[test]
fn settings() {
    let (gpp, mock) = open(IDN_4323);
    let keys: Vec<String> = gpp.settings().into_iter().map(|s| s.key).collect();
    assert_eq!(keys.len(), 1 + 4 * 4);
    assert_eq!(
        &keys[..5],
        ["poll_rate", "ovp1", "ovp1_on", "ocp1", "ocp1_on"]
    );
    let ovp4 = gpp
        .settings()
        .into_iter()
        .find(|s| s.key == "ovp4")
        .unwrap();
    assert_eq!(
        (ovp4.label.as_str(), ovp4.output),
        ("Overvoltage limit", Some(4))
    );
    assert_eq!(gpp.get("poll_rate").unwrap(), Value::from(10.0));
    assert_eq!(gpp.get("ovp1").unwrap(), Value::from(35.0));
    assert_eq!(gpp.get("ocp1").unwrap(), Value::from(3.5));
    assert_eq!(gpp.get("ovp1_on").unwrap(), Value::from(false));
    gpp.set("ovp1", 30.0.into()).unwrap();
    gpp.set("ocp2", 1.234.into()).unwrap();
    gpp.set("ocp2_on", true.into()).unwrap();
    gpp.set("ovp3_on", false.into()).unwrap();
    let text = mock.text();
    for sent in [
        ":OUTPut1:OVP 30.0\n:SYSTem:ERRor?\n",
        ":OUTPut2:OCP 1.23\n:SYSTem:ERRor?\n",
        ":OUTPut2:OCP:STATe ON\n:SYSTem:ERRor?\n",
        ":OUTPut3:OVP:STATe OFF\n:SYSTem:ERRor?\n",
    ] {
        assert!(text.contains(sent), "{sent:?}");
    }
    assert_eq!(gpp.get("ovp1").unwrap(), Value::from(30.0));
    assert_eq!(gpp.get("ocp2_on").unwrap(), Value::from(true));
    gpp.set("poll_rate", 20.into()).unwrap();
    assert_eq!(gpp.get("poll_rate").unwrap(), Value::from(20.0));
    assert!(gpp.channels().iter().all(|c| c.rate_hz == 20));
}

#[test]
fn polling() {
    let mut v = values(IDN_4323);
    v.insert(
        ":MEASure:VOLTage:ALL".into(),
        "5.0012,12.0000,0.0000,0.0000".into(),
    );
    v.insert(
        ":MEASure:CURRent:ALL".into(),
        "0.0003,0.5000,0.0000,0.0000".into(),
    );
    let (gpp, _mock) = open_with(v);
    let events = gpp.events();
    let mut seen = HashMap::new();
    let deadline = Instant::now() + WAIT;
    while seen.len() < 12 && Instant::now() < deadline {
        if let Ok(Event::Reading { ch, value }) = events.recv_timeout(WAIT) {
            seen.insert(ch, value);
        }
    }
    assert_eq!(seen.len(), 12, "every channel of all four outputs");
    assert_eq!(seen[&id(&gpp, "ch1.v")], 5.0012);
    assert_eq!(seen[&id(&gpp, "ch1.p")], (5.0012f64 * 0.0003) as f32);
    assert_eq!(seen[&id(&gpp, "ch2.p")], 6.0);

    let (tx, blocks) = mpsc::channel();
    let (v2, p2) = (id(&gpp, "ch2.v"), id(&gpp, "ch2.p"));
    let meter = gpp.meter().unwrap();
    assert_eq!(
        meter.start(&[999], tx.clone()).unwrap_err().kind,
        ErrorKind::Invalid
    );
    meter.start(&[v2, p2], tx).unwrap();
    let got: Vec<Block> = (0..4).map(|_| blocks.recv_timeout(WAIT).unwrap()).collect();
    assert_eq!(
        got[..2],
        [
            Block {
                ch: v2,
                t: 0.0,
                dt: 0.1,
                data: Data::Analog(vec![12.0])
            },
            Block {
                ch: p2,
                t: 0.0,
                dt: 0.1,
                data: Data::Analog(vec![6.0])
            },
        ]
    );
    assert_eq!(got[2].t, got[3].t);
    assert!(got[2].t > 0.05 && got[2].t < 0.5, "{}", got[2].t);
    meter.stop().unwrap();
    while blocks.recv_timeout(WAIT).is_ok() {}
}

#[test]
fn unanswered_query_times_out_cleanly() {
    let mut v = values(IDN_4323);
    v.insert(":OUTPut1:OVP".into(), "late:35.0".into());
    v.insert(":OUTPut1:OCP".into(), "3.50".into());
    let (gpp, mock) = open_with(v);
    let t0 = Instant::now();
    let e = gpp.get("ovp1").unwrap_err();
    assert_eq!(e.kind, ErrorKind::Timeout);
    assert!(t0.elapsed() >= Duration::from_millis(500));
    assert_eq!(gpp.get("ocp1").unwrap(), Value::from(3.5));
    // The late reply has arrived by now and must not answer the next query.
    thread::sleep(Duration::from_millis(400));
    assert_eq!(gpp.get("ocp1").unwrap(), Value::from(3.5));
    mock.set(":OUTPut1:OVP", "34.0");
    assert_eq!(gpp.get("ovp1").unwrap(), Value::from(34.0));
    // An unknown header gets no reply at all.
    mock.values.lock().unwrap().remove(":OUTPut2:OVP");
    assert_eq!(gpp.get("ovp2").unwrap_err().kind, ErrorKind::Timeout);
    assert_eq!(gpp.get("ocp1").unwrap(), Value::from(3.5));
}

#[test]
fn local_on_drop() {
    let (gpp, mock) = open(IDN_4323);
    gpp.supply().unwrap().state(1).unwrap();
    drop(gpp);
    let deadline = Instant::now() + WAIT;
    while mock.lines().last().map(String::as_str) != Some("LOCAL") {
        assert!(Instant::now() < deadline, "{:?}", mock.lines());
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(mock.lines().iter().filter(|l| *l == "LOCAL").count(), 1);
}

#[test]
fn disconnect() {
    let (gpp, mock) = open(IDN_4323);
    let events = gpp.events();
    let (tx, blocks) = mpsc::channel();
    gpp.meter()
        .unwrap()
        .start(&[id(&gpp, "ch1.v")], tx)
        .unwrap();
    blocks.recv_timeout(WAIT).unwrap();
    mock.server.shutdown(Shutdown::Both).unwrap();
    let deadline = Instant::now() + WAIT;
    loop {
        match events.recv_timeout(WAIT) {
            Ok(Event::Disconnected) => break,
            Ok(_) => assert!(Instant::now() < deadline),
            Err(e) => panic!("{e}"),
        }
    }
    while blocks.recv_timeout(WAIT).is_ok() {}
    assert_eq!(
        gpp.supply().unwrap().state(1).unwrap_err().kind,
        ErrorKind::Io
    );
    assert_eq!(
        gpp.meter()
            .unwrap()
            .start(&[], mpsc::channel().0)
            .unwrap_err()
            .kind,
        ErrorKind::Closed
    );
}
