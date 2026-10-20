use clap::{Parser, Subcommand};
use otii_arc::{
    Arc, Event,
    prop::{self, PropertyValue, ValueKind},
};
use otii_rec::{
    ChannelSpec, Kind, Recording, export,
    file::{DeviceInfo, Meta},
};
use serde_json::{Value, json};
use std::{
    io::{self, Write},
    path::Path,
    sync::{
        Arc as Shared,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

#[derive(Parser)]
#[command(name = "otii", version)]
struct Cli {
    #[arg(short, long, global = true)]
    port: Option<String>,
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    Info,
    Get {
        name: String,
    },
    Set {
        name: String,
        value: String,
    },
    On,
    Off,
    Volt {
        v: String,
    },
    Ilim {
        a: String,
    },
    Monitor,
    Power {
        #[arg(long)]
        volt: Option<String>,
        #[arg(long)]
        ilim: Option<String>,
        #[arg(long)]
        mode: Option<String>,
        state: Option<String>,
    },
    Measure {
        #[arg(long)]
        ch: String,
        #[arg(long)]
        duration: String,
        #[arg(long = "assert")]
        assertions: Vec<String>,
    },
    Record {
        #[arg(long)]
        ch: String,
        #[arg(long)]
        duration: Option<String>,
        #[arg(long)]
        secs: Option<u64>,
        #[arg(short, long, default_value = "out.csv")]
        output: String,
        #[arg(long)]
        uart: Option<u32>,
        #[arg(long)]
        gpi: bool,
    },
    Convert {
        input: String,
        output: String,
    },
    Stats {
        input: String,
        #[arg(long)]
        from: Option<String>,
        #[arg(long)]
        to: Option<String>,
    },
    Uart {
        #[arg(long, default_value_t = 115200)]
        baud: u32,
        #[arg(long)]
        send: Option<String>,
        #[arg(long, default_value_t = 2)]
        secs: u64,
    },
    Gpo {
        n: u8,
        state: String,
    },
}
#[derive(Debug)]
struct Failure {
    code: i32,
    message: String,
}
impl Failure {
    fn usage(s: impl Into<String>) -> Self {
        Self {
            code: 3,
            message: s.into(),
        }
    }
    fn io(s: impl Into<String>) -> Self {
        Self {
            code: 2,
            message: s.into(),
        }
    }
}
type Result<T> = std::result::Result<T, Failure>;
fn main() {
    let cli = match Cli::try_parse() {
        Ok(c) => c,
        Err(e) => {
            let json = std::env::args().any(|a| a == "--json");
            let msg = e.to_string();
            if json {
                eprintln!("{}", json!({"error":msg}));
            } else {
                eprintln!("{msg}");
            }
            std::process::exit(3)
        }
    };
    let is_json = cli.json;
    match run(cli) {
        Ok((value, code)) => {
            if is_json {
                println!("{value}");
            } else {
                print_human(&value);
            }
            std::process::exit(code)
        }
        Err(e) => {
            if is_json {
                eprintln!("{}", json!({"error":e.message}));
            } else {
                eprintln!("{}", e.message);
            }
            std::process::exit(e.code)
        }
    }
}
fn print_human(v: &Value) {
    if let Some(s) = v.as_str() {
        println!("{s}");
    } else {
        println!("{}", serde_json::to_string_pretty(v).unwrap());
    }
}
fn parse_number(s: &str) -> Result<f64> {
    let s = s.trim();
    let cut = s
        .char_indices()
        .find(|(_, c)| !c.is_ascii_digit() && !matches!(c, '.' | '-' | '+' | 'e' | 'E'))
        .map_or(s.len(), |(i, _)| i);
    let n: f64 = s[..cut]
        .parse()
        .map_err(|_| Failure::usage(format!("invalid value {s}")))?;
    if !n.is_finite() {
        return Err(Failure::usage("value must be finite"));
    }
    let scale = match &s[cut..] {
        "" | "V" | "A" | "W" | "s" => 1.0,
        "mV" | "mA" | "mW" | "ms" => 0.001,
        "uV" | "µV" | "uA" | "µA" | "uW" | "µW" => 1e-6,
        "min" => 60.0,
        "mAh" | "mWh" => 0.001,
        "Ah" | "Wh" => 1.0,
        _ => return Err(Failure::usage(format!("unknown unit in {s}"))),
    };
    Ok(n * scale)
}
fn value_unit(s: &str, unit: &str) -> Result<f64> {
    let v = parse_number(s)?;
    let suffix = s.trim().trim_start_matches(|c: char| {
        c.is_ascii_digit() || matches!(c, '.' | '-' | '+' | 'e' | 'E')
    });
    if suffix.is_empty() || suffix.ends_with(unit) {
        Ok(v)
    } else {
        Err(Failure::usage(format!("expected {unit}: {s}")))
    }
}
fn duration(s: &str) -> Result<Duration> {
    let v = value_unit(s, "s")?;
    if v < 0.0 {
        return Err(Failure::usage("negative duration"));
    }
    if v > u64::MAX as f64 {
        return Err(Failure::usage("duration too large"));
    }
    Ok(Duration::from_secs_f64(v))
}
fn parse_bool(s: &str) -> Result<bool> {
    match s.to_ascii_lowercase().as_str() {
        "on" | "true" | "1" => Ok(true),
        "off" | "false" | "0" => Ok(false),
        _ => Err(Failure::usage(format!("expected on/off: {s}"))),
    }
}
fn parse_prop(s: &str) -> Result<u32> {
    if let Some(x) = s.strip_prefix("0x") {
        u32::from_str_radix(x, 16).map_err(|_| Failure::usage("invalid property id"))
    } else {
        prop::property_by_name(s)
            .map(|p| p.id)
            .ok_or_else(|| Failure::usage(format!("unknown property {s}")))
    }
}
fn device(cli: &Cli) -> Result<Arc> {
    let port = cli
        .port
        .clone()
        .or_else(otii_arc::device::find_port)
        .ok_or_else(|| Failure::io("no Arc serial port found"))?;
    Arc::open(&port).map_err(|e| Failure::io(e.to_string()))
}
fn dev<T>(r: std::result::Result<T, otii_arc::device::Error>) -> Result<T> {
    r.map_err(|e| Failure::io(e.to_string()))
}
fn specs(arc: &Arc, ch: &str, uart: bool, gpi: bool) -> Result<Vec<ChannelSpec>> {
    let info = arc.info();
    let mut ids: Vec<u16> = ch
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| {
            prop::channel_by_name(s).ok_or_else(|| Failure::usage(format!("unknown channel {s}")))
        })
        .collect::<Result<_>>()?;
    if uart && !ids.contains(&0x17) {
        ids.push(0x17)
    }
    if gpi && !ids.contains(&0x16) {
        ids.push(0x16)
    }
    if ids.is_empty() {
        return Err(Failure::usage("no channels selected"));
    }
    ids.sort_unstable();
    ids.dedup();
    Ok(ids
        .into_iter()
        .map(|id| {
            let ci = info.chinfo.get(id);
            let kind = if id == 0x17 {
                Kind::Bytes
            } else if id == 0x16 {
                Kind::Digital
            } else {
                Kind::Analog
            };
            ChannelSpec {
                id,
                name: prop::channel_name(id).into(),
                unit: unit(id).into(),
                rate_hz: ci.rate_hz,
                kind,
            }
        })
        .collect())
}
fn unit(ch: u16) -> &'static str {
    match ch {
        0 | 2 | 0xc..=0xe => "A",
        1 | 3 | 5 | 0x10..=0x13 => "V",
        6 | 7 => "W",
        0x14 => "°C",
        _ => "",
    }
}
fn stop_flag() -> Result<Shared<AtomicBool>> {
    let run = Shared::new(AtomicBool::new(true));
    let flag = run.clone();
    ctrlc::set_handler(move || flag.store(false, Ordering::Relaxed))
        .map_err(|e| Failure::io(e.to_string()))?;
    Ok(run)
}
fn capture(
    arc: &Arc,
    selected: &[ChannelSpec],
    for_time: Duration,
    uart: Option<u32>,
) -> Result<Recording> {
    let run = stop_flag()?;
    let rx = arc.events();
    if let Some(baud) = uart {
        dev(arc.uart_open(baud))?;
    }
    let ids: Vec<u32> = selected.iter().map(|s| u32::from(s.id)).collect();
    if let Err(e) = arc.start_recording(&ids) {
        if uart.is_some() {
            let _ = arc.uart_close();
        }
        return Err(Failure::io(e.to_string()));
    }
    let mut rec = Recording::new(selected);
    let deadline = Instant::now() + for_time;
    while run.load(Ordering::Relaxed) && Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(Event::Data { ms, block }) => rec.push_block(ms, &block),
            Ok(Event::Disconnected) => break,
            _ => {}
        }
    }
    let stopped = arc.stop_recording();
    if uart.is_some() {
        let _ = arc.uart_close();
    }
    rec.stop();
    dev(stopped)?;
    Ok(rec)
}
fn stats_value(rec: &Recording, from: f64, to: f64) -> Value {
    let mut channels = serde_json::Map::new();
    for spec in rec.specs() {
        if spec.kind != Kind::Analog {
            continue;
        }
        if let Some(s) = rec.stats(spec.id, from, to) {
            channels.insert(spec.name,json!({"unit":spec.unit,"count":s.count,"mean":s.mean,"min":s.min,"max":s.max,"rms":s.rms,"p_p":s.max-s.min,"integral":s.integral}));
        }
    }
    json!({"duration_s":rec.duration(),"from_s":from,"to_s":to,"channels":channels,"charge_mAh":rec.charge_mah(from,to),"energy":rec.energy(from,to),"gaps":rec.gaps()})
}
fn write_format(rec: &Recording, path: &str, meta: &Meta) -> Result<()> {
    let p = Path::new(path);
    match p.extension().and_then(|s| s.to_str()) {
        Some("otii-rec") => rec.save(p, meta).map_err(|e| Failure::io(e.to_string())),
        Some("csv") => export::export_csv(rec, p, None).map_err(|e| Failure::io(e.to_string())),
        Some("sr") => export::export_sr(rec, p).map_err(|e| Failure::io(e.to_string())),
        _ => Err(Failure::usage("output must end in .otii-rec, .csv or .sr")),
    }
}
#[derive(Debug)]
struct Assertion {
    target: String,
    op: String,
    limit: f64,
}
fn parse_assertion(s: &str) -> Result<Assertion> {
    let (at, op) = s
        .find("<=")
        .map(|i| (i, "<="))
        .or_else(|| s.find(">=").map(|i| (i, ">=")))
        .or_else(|| s.find('<').map(|i| (i, "<")))
        .or_else(|| s.find('>').map(|i| (i, ">")))
        .ok_or_else(|| Failure::usage(format!("invalid assertion {s}")))?;
    let target = s[..at].trim();
    if target.is_empty() {
        return Err(Failure::usage("empty assertion target"));
    }
    Ok(Assertion {
        target: target.into(),
        op: op.into(),
        limit: parse_number(s[at + op.len()..].trim())?,
    })
}
fn check_assertions(rec: &Recording, items: &[String]) -> Result<(Value, bool)> {
    let mut reports = Vec::new();
    let mut all = true;
    let stats = stats_value(rec, 0.0, rec.duration());
    for raw in items {
        let a = parse_assertion(raw)?;
        let actual = if a.target == "charge" {
            stats["charge_mAh"].as_f64().map(|x| x * 0.001)
        } else if a.target == "energy" {
            stats["energy"]["mwh"].as_f64().map(|x| x * 0.001)
        } else if let Some((ch, field)) = a.target.split_once('.') {
            stats["channels"][ch][field].as_f64()
        } else {
            None
        }
        .ok_or_else(|| Failure::usage(format!("unknown assertion target {}", a.target)))?;
        let passed = match a.op.as_str() {
            "<" => actual < a.limit,
            "<=" => actual <= a.limit,
            ">" => actual > a.limit,
            ">=" => actual >= a.limit,
            _ => false,
        };
        all &= passed;
        reports.push(json!({"expression":raw,"actual":actual,"limit":a.limit,"passed":passed}));
    }
    Ok((Value::Array(reports), all))
}
fn validate_live(command: &Command) -> Result<()> {
    match command {
        Command::Get { name } => {
            parse_prop(name)?;
        }
        Command::Set { name, value } => {
            parse_prop(name)?;
            if let Some(p) = prop::property(parse_prop(name)?) {
                match p.kind {
                    ValueKind::String => {}
                    ValueKind::Bool => {
                        parse_bool(value)?;
                    }
                    _ => {
                        parse_number(value)?;
                    }
                }
            }
        }
        Command::Volt { v } => {
            value_unit(v, "V")?;
        }
        Command::Ilim { a } => {
            value_unit(a, "A")?;
        }
        Command::Power {
            volt,
            ilim,
            mode,
            state,
        } => {
            if let Some(v) = volt {
                value_unit(v, "V")?;
            }
            if let Some(a) = ilim {
                value_unit(a, "A")?;
            }
            if let Some(m) = mode
                && m != "cutoff"
                && m != "limit"
            {
                return Err(Failure::usage("mode must be cutoff or limit"));
            }
            if let Some(s) = state {
                parse_bool(s)?;
            }
        }
        Command::Measure {
            ch,
            duration: d,
            assertions,
        } => {
            duration(d)?;
            let names: Vec<_> = ch.split(',').map(str::trim).collect();
            for name in &names {
                if prop::channel_by_name(name).is_none() {
                    return Err(Failure::usage(format!("unknown channel {name}")));
                }
            }
            for raw in assertions {
                let a = parse_assertion(raw)?;
                if a.target != "charge" && a.target != "energy" {
                    let (name, field) = a.target.split_once('.').ok_or_else(|| {
                        Failure::usage(format!("invalid assertion target {}", a.target))
                    })?;
                    if !names.contains(&name)
                        || !["mean", "min", "max", "rms", "p_p", "integral"].contains(&field)
                    {
                        return Err(Failure::usage(format!(
                            "invalid assertion target {}",
                            a.target
                        )));
                    }
                }
            }
        }
        Command::Record {
            ch,
            duration: d,
            secs,
            output,
            ..
        } => {
            if let Some(d) = d {
                duration(d)?;
            } else if secs.is_none() {
                return Err(Failure::usage("--duration required"));
            }
            for name in ch.split(',').map(str::trim) {
                if prop::channel_by_name(name).is_none() {
                    return Err(Failure::usage(format!("unknown channel {name}")));
                }
            }
            check_extension(output)?;
        }
        Command::Gpo { n, state } => {
            if !(1..=3).contains(n) {
                return Err(Failure::usage("GPO must be 1..=3"));
            }
            parse_bool(state)?;
        }
        _ => {}
    }
    Ok(())
}
fn check_extension(path: &str) -> Result<()> {
    match Path::new(path).extension().and_then(|s| s.to_str()) {
        Some("otii-rec" | "csv" | "sr") => Ok(()),
        _ => Err(Failure::usage("output must end in .otii-rec, .csv or .sr")),
    }
}
fn run(cli: Cli) -> Result<(Value, i32)> {
    match &cli.command {
        Command::Convert { input, output } => {
            check_extension(output)?;
            let (rec, meta) = Recording::load(input).map_err(|e| Failure::io(e.to_string()))?;
            write_format(&rec, output, &meta)?;
            return Ok((json!({"output":output,"samples":rec.sample_counts()}), 0));
        }
        Command::Stats { input, from, to } => {
            let (rec, _) = Recording::load(input).map_err(|e| Failure::io(e.to_string()))?;
            let f = from
                .as_deref()
                .map(duration)
                .transpose()?
                .map_or(0.0, |d| d.as_secs_f64());
            let t = to
                .as_deref()
                .map(duration)
                .transpose()?
                .map_or(rec.duration(), |d| d.as_secs_f64());
            if t < f {
                return Err(Failure::usage("--to before --from"));
            }
            return Ok((stats_value(&rec, f, t), 0));
        }
        _ => {}
    }
    validate_live(&cli.command)?;
    let arc = device(&cli)?;
    let value = match cli.command {
        Command::Info => {
            let i = arc.info();
            let mut props = serde_json::Map::new();
            for (id, val) in i.properties {
                props.insert(
                    prop::property(id).map_or(format!("0x{id:x}"), |p| p.name.into()),
                    json!(val.to_string()),
                );
            }
            json!({"protocol":i.proto,"max_payload":i.max_len,"device_type":i.devtype,"features":i.featurebits,"properties":props})
        }
        Command::Get { name } => {
            let id = parse_prop(&name)?;
            let v = dev(arc.get_value(id))?;
            json!({"name":name,"value":v.to_string()})
        }
        Command::Set { name, value } => {
            let id = parse_prop(&name)?;
            let p = prop::property(id).ok_or_else(|| Failure::usage("unknown property"))?;
            let v = match p.kind {
                ValueKind::String => PropertyValue::String(value),
                ValueKind::Bool => PropertyValue::Bool(parse_bool(&value)?),
                ValueKind::Int => PropertyValue::Int(parse_number(&value)? as i32),
                ValueKind::Enum => PropertyValue::Enum(parse_number(&value)? as i32),
                ValueKind::Scaled => PropertyValue::Number(parse_number(&value)?),
            };
            dev(arc.set(id, &v))?;
            json!({"name":name,"value":v.to_string()})
        }
        Command::On => {
            dev(arc.output(true))?;
            json!({"power":"on"})
        }
        Command::Off => {
            dev(arc.output(false))?;
            json!({"power":"off"})
        }
        Command::Volt { v } => {
            let n = value_unit(&v, "V")?;
            dev(arc.set_voltage(n))?;
            json!({"voltage_V":n})
        }
        Command::Ilim { a } => {
            let n = value_unit(&a, "A")?;
            dev(arc.set_current_limit(n))?;
            json!({"current_limit_A":n})
        }
        Command::Power {
            volt,
            ilim,
            mode,
            state,
        } => {
            if let Some(v) = volt {
                dev(arc.set_voltage(value_unit(&v, "V")?))?
            }
            if let Some(a) = ilim {
                dev(arc.set_current_limit(value_unit(&a, "A")?))?
            }
            if let Some(m) = mode {
                let id = match m.as_str() {
                    "cutoff" => 0,
                    "limit" => 1,
                    _ => return Err(Failure::usage("mode must be cutoff or limit")),
                };
                dev(arc.set(6, &PropertyValue::Enum(id)))?
            }
            if let Some(s) = state {
                dev(arc.output(parse_bool(&s)?))?
            }
            json!({"power":"configured"})
        }
        Command::Measure {
            ch,
            duration: d,
            assertions,
        } => {
            let specs = specs(&arc, &ch, false, false)?;
            let rec = capture(&arc, &specs, duration(&d)?, None)?;
            let (mut summary, mut code) = (stats_value(&rec, 0.0, rec.duration()), 0);
            let (reports, all) = check_assertions(&rec, &assertions)?;
            summary["assertions"] = reports;
            if !all {
                code = 1;
            }
            return Ok((summary, code));
        }
        Command::Record {
            ch,
            duration: d,
            secs,
            output,
            uart,
            gpi,
        } => {
            let duration = if let Some(d) = d {
                duration(&d)?
            } else if let Some(s) = secs {
                Duration::from_secs(s)
            } else {
                return Err(Failure::usage("--duration required"));
            };
            let specs = specs(&arc, &ch, uart.is_some(), gpi)?;
            let rec = capture(&arc, &specs, duration, uart)?;
            let info = arc.info();
            let property = |id| info.properties.get(&id).map(ToString::to_string);
            let meta = Meta {
                device: DeviceInfo {
                    name: property(1),
                    fw: property(0x81),
                    hw: property(0x80),
                    uid: property(0x82),
                },
                ..Meta::default()
            };
            write_format(&rec, &output, &meta)?;
            json!({"output":output,"duration_s":rec.duration(),"samples":rec.sample_counts(),"gaps":rec.gaps()})
        }
        Command::Monitor => {
            let run = stop_flag()?;
            let rx = arc.events();
            while run.load(Ordering::Relaxed) {
                match rx.recv_timeout(Duration::from_millis(100)) {
                    Ok(Event::Monitor { ch, value }) => {
                        let v = json!({"channel":prop::channel_name(ch as u16),"value":value});
                        if cli.json {
                            println!("{v}")
                        } else {
                            print_human(&v)
                        }
                    }
                    Ok(Event::Gpi(mask)) => {
                        let v = json!({"gpi":mask});
                        if cli.json {
                            println!("{v}")
                        } else {
                            print_human(&v)
                        }
                    }
                    _ => {}
                }
            }
            return Ok((json!({"stopped":true}), 0));
        }
        Command::Uart { baud, send, secs } => {
            let rx = arc.events();
            dev(arc.uart_open(baud))?;
            dev(arc.start_recording(&[0x17]))?;
            if let Some(s) = send {
                dev(arc.uart_write(s.as_bytes()))?
            }
            let deadline = Instant::now() + Duration::from_secs(secs);
            let mut bytes = Vec::new();
            while Instant::now() < deadline {
                if let Ok(Event::Data { block, .. }) = rx.recv_timeout(Duration::from_millis(100))
                    && let otii_arc::msg::BlockBody::Bytes(v) = block.body
                {
                    bytes.extend(v)
                }
            }
            dev(arc.stop_recording())?;
            dev(arc.uart_close())?;
            if !cli.json {
                io::stdout()
                    .write_all(&bytes)
                    .map_err(|e| Failure::io(e.to_string()))?;
            }
            json!({"rx_bytes":bytes})
        }
        Command::Gpo { n, state } => {
            dev(arc.set_gpo(n, parse_bool(&state)?))?;
            json!({"gpo":n,"state":state})
        }
        Command::Convert { .. } | Command::Stats { .. } => unreachable!(),
    };
    Ok((value, 0))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn units() {
        assert!((parse_number("3300mV").unwrap() - 3.3).abs() < 1e-12);
        assert!((parse_number("50µA").unwrap() - 50e-6).abs() < 1e-12);
        assert_eq!(parse_number("1min").unwrap(), 60.0);
        assert!(parse_number("3bad").is_err());
    }
    #[test]
    fn assertions() {
        let a = parse_assertion("mc.mean<=50uA").unwrap();
        assert_eq!(a.target, "mc.mean");
        assert_eq!(a.op, "<=");
        assert!((a.limit - 50e-6).abs() < 1e-12);
    }
    #[test]
    fn fixture_offline_convert_and_stats() {
        use otii_arc::{
            msg::{self, Message},
            prop::ChannelTable,
        };
        let hex = |line: &str| -> Vec<u8> {
            line.as_bytes()
                .chunks_exact(2)
                .map(|b| u8::from_str_radix(std::str::from_utf8(b).unwrap(), 16).unwrap())
                .collect()
        };
        let mut table = ChannelTable::default();
        let selected = [
            ChannelSpec {
                id: 0,
                name: "mc".into(),
                unit: "A".into(),
                rate_hz: 4000,
                kind: Kind::Analog,
            },
            ChannelSpec {
                id: 1,
                name: "mv".into(),
                unit: "V".into(),
                rate_hz: 1000,
                kind: Kind::Analog,
            },
        ];
        let mut rec = Recording::new(&selected);
        for line in include_str!("../../../fixtures/session1.hex")
            .lines()
            .filter(|s| !s.is_empty() && !s.starts_with('#'))
        {
            match msg::parse(&hex(line), &table).unwrap() {
                Message::CmdResp { txn: 14, body, .. } => table = ChannelTable::decode(&body),
                Message::Data { ms, blocks } => {
                    for b in blocks {
                        rec.push_block(ms, &b)
                    }
                }
                _ => {}
            }
        }
        let base = std::env::temp_dir().join(format!("otii-cli-fixture-{}", std::process::id()));
        let native = base.with_extension("otii-rec");
        let csv = base.with_extension("csv");
        rec.save(&native, &Meta::default()).unwrap();
        let (converted, code) = run(Cli {
            port: None,
            json: true,
            command: Command::Convert {
                input: native.to_str().unwrap().into(),
                output: csv.to_str().unwrap().into(),
            },
        })
        .unwrap();
        assert_eq!(code, 0);
        assert_eq!(converted["samples"]["mc"], rec.sample_counts()["mc"]);
        let (stats, code) = run(Cli {
            port: None,
            json: true,
            command: Command::Stats {
                input: native.to_str().unwrap().into(),
                from: None,
                to: None,
            },
        })
        .unwrap();
        assert_eq!(code, 0);
        assert!(
            stats["channels"]["mc"]["mean"]
                .as_f64()
                .unwrap()
                .is_finite()
        );
        assert!(
            std::fs::read_to_string(&csv)
                .unwrap()
                .starts_with("time_s,mc[A],mv[V]")
        );
        std::fs::remove_file(native).unwrap();
        std::fs::remove_file(csv).unwrap();
    }
}
