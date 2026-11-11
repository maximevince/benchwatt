mod human;

use benchwatt_session::{
    Capture,
    device::{Device, ErrorKind, Event, Gpio, Supply},
    rec::{Kind, Recording, export, file::Meta},
};
use clap::{Parser, Subcommand};
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
#[command(name = "benchwatt", version)]
struct Cli {
    /// Which device: its kind, or `kind@port` when it cannot be found by USB id.
    #[arg(short, long, global = true)]
    device: Option<String>,
    /// Serial port of the Arc; the same as `--device arc@PORT`.
    #[arg(short, long, global = true)]
    port: Option<String>,
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    /// List the supported devices that are plugged in.
    Devices,
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
        /// Which output, on a supply with several.
        #[arg(long, default_value_t = 1)]
        out: u8,
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
    let s = human::render(v);
    if !s.is_empty() {
        println!("{s}");
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
fn device(cli: &Cli) -> Result<Box<dyn Device>> {
    let by_port = cli.port.as_ref().map(|p| format!("arc@{p}"));
    let found = dev(benchwatt_session::select(
        cli.device.as_deref().or(by_port.as_deref()),
    ))?;
    dev(benchwatt_session::open(&found))
}
fn dev<T>(r: benchwatt_session::device::Result<T>) -> Result<T> {
    r.map_err(|e| match e.kind {
        ErrorKind::Invalid => Failure::usage(e.message),
        _ => Failure::io(e.message),
    })
}
fn supply(d: &dyn Device) -> Result<&dyn Supply> {
    d.supply()
        .ok_or_else(|| Failure::usage("device has no supply output"))
}
fn gpio(d: &dyn Device) -> Result<&dyn Gpio> {
    d.gpio()
        .ok_or_else(|| Failure::usage("device has no digital outputs"))
}
/// A device value as `get` and `set` print it.
fn text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) if n.is_f64() => n.as_f64().unwrap().to_string(),
        _ => v.to_string(),
    }
}
fn names(ch: &str) -> Vec<&str> {
    ch.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect()
}
fn stop_flag() -> Result<Shared<AtomicBool>> {
    let run = Shared::new(AtomicBool::new(true));
    let flag = run.clone();
    ctrlc::set_handler(move || flag.store(false, Ordering::Relaxed))
        .map_err(|e| Failure::io(e.to_string()))?;
    Ok(run)
}
fn capture(
    d: &dyn Device,
    ch: &str,
    gpi: bool,
    uart: Option<u32>,
    for_time: Duration,
) -> Result<Recording> {
    let run = stop_flag()?;
    let (capture, mut rec) = dev(Capture::start(d, &names(ch), gpi, uart))?;
    let deadline = Instant::now() + for_time;
    while run.load(Ordering::Relaxed)
        && Instant::now() < deadline
        && capture.pump(&mut rec, Duration::from_millis(50))
    {}
    dev(capture.stop(d, &mut rec))?;
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
        Some("bwrec") => rec.save(p, meta).map_err(|e| Failure::io(e.to_string())),
        Some("csv") => export::export_csv(rec, p, None).map_err(|e| Failure::io(e.to_string())),
        Some("sr") => export::export_sr(rec, p).map_err(|e| Failure::io(e.to_string())),
        _ => Err(Failure::usage("output must end in .bwrec, .csv or .sr")),
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
        Command::Volt { v } => {
            value_unit(v, "V")?;
        }
        Command::Ilim { a } => {
            value_unit(a, "A")?;
        }
        Command::Power {
            volt, ilim, state, ..
        } => {
            if let Some(v) = volt {
                value_unit(v, "V")?;
            }
            if let Some(a) = ilim {
                value_unit(a, "A")?;
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
            let names = names(ch);
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
            check_extension(output)?;
        }
        Command::Gpo { n, state } => {
            if !(1..=2).contains(n) {
                return Err(Failure::usage("GPO must be 1 or 2"));
            }
            parse_bool(state)?;
        }
        _ => {}
    }
    Ok(())
}
fn check_extension(path: &str) -> Result<()> {
    match Path::new(path).extension().and_then(|s| s.to_str()) {
        Some("bwrec" | "csv" | "sr") => Ok(()),
        _ => Err(Failure::usage("output must end in .bwrec, .csv or .sr")),
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
        Command::Devices => {
            let list: Vec<_> = benchwatt_session::find()
                .iter()
                .map(|f| json!({"kind":f.kind,"port":f.port}))
                .collect();
            return Ok((json!({"devices":list}), 0));
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
    let d = device(&cli)?;
    let d = d.as_ref();
    let value = match cli.command {
        Command::Info => {
            // The identity every device has, then whatever else the driver knows.
            let info = d.info();
            let mut out = serde_json::Map::new();
            for (key, value) in [
                ("kind", info.kind),
                ("model", info.model),
                ("serial", info.serial),
                ("firmware", info.firmware),
                ("hardware", info.hardware),
            ] {
                if !value.is_empty() {
                    out.insert(key.into(), value.into());
                }
            }
            out.extend(info.details);
            Value::Object(out)
        }
        Command::Get { name } => {
            let v = dev(d.get(&name))?;
            json!({"name":name,"value":text(&v)})
        }
        Command::Set { name, value } => {
            // The value's type follows what the device holds for that key now.
            let v = match dev(d.get(&name))? {
                Value::String(_) => Value::String(value),
                Value::Bool(_) => Value::Bool(parse_bool(&value)?),
                Value::Number(n) if n.is_f64() => json!(parse_number(&value)?),
                _ => json!(parse_number(&value)? as i32),
            };
            dev(d.set(&name, v.clone()))?;
            json!({"name":name,"value":text(&v)})
        }
        Command::On => {
            dev(supply(d)?.set_enabled(1, true))?;
            json!({"output":true})
        }
        Command::Off => {
            dev(supply(d)?.set_enabled(1, false))?;
            json!({"output":false})
        }
        Command::Volt { v } => {
            let n = value_unit(&v, "V")?;
            dev(supply(d)?.set_voltage(1, n))?;
            json!({"voltage_V":n})
        }
        Command::Ilim { a } => {
            let n = value_unit(&a, "A")?;
            dev(supply(d)?.set_current_limit(1, n))?;
            json!({"current_limit_A":n})
        }
        Command::Power {
            volt,
            ilim,
            mode,
            out,
            state,
        } => {
            let mut applied = serde_json::Map::new();
            if let Some(v) = volt {
                let n = value_unit(&v, "V")?;
                dev(supply(d)?.set_voltage(out, n))?;
                applied.insert("voltage_V".into(), json!(n));
            }
            if let Some(a) = ilim {
                let n = value_unit(&a, "A")?;
                dev(supply(d)?.set_current_limit(out, n))?;
                applied.insert("current_limit_A".into(), json!(n));
            }
            if let Some(m) = mode {
                dev(d.set("current_mode", json!(m)))?;
                applied.insert("mode".into(), json!(m));
            }
            if let Some(s) = state {
                let on = parse_bool(&s)?;
                dev(supply(d)?.set_enabled(out, on))?;
                applied.insert("output".into(), json!(on));
            }
            Value::Object(applied)
        }
        Command::Measure {
            ch,
            duration: t,
            assertions,
        } => {
            let rec = capture(d, &ch, false, None, duration(&t)?)?;
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
            duration: t,
            secs,
            output,
            uart,
            gpi,
        } => {
            let duration = if let Some(t) = t {
                duration(&t)?
            } else if let Some(s) = secs {
                Duration::from_secs(s)
            } else {
                return Err(Failure::usage("--duration required"));
            };
            let rec = capture(d, &ch, gpi, uart, duration)?;
            write_format(&rec, &output, &benchwatt_session::meta(d))?;
            json!({"output":output,"duration_s":rec.duration(),"samples":rec.sample_counts(),"gaps":rec.gaps()})
        }
        Command::Monitor => {
            let run = stop_flag()?;
            let rx = d.events();
            let channels = d.channels();
            while run.load(Ordering::Relaxed) {
                let v = match rx.recv_timeout(Duration::from_millis(100)) {
                    Ok(Event::Reading { ch, value }) => {
                        let name = channels.iter().find(|c| c.id == ch).map_or("", |c| &c.name);
                        json!({"channel":name,"value":value})
                    }
                    Ok(Event::Inputs(mask)) => json!({"gpi":mask}),
                    _ => continue,
                };
                if cli.json {
                    println!("{v}")
                } else {
                    print_human(&v)
                }
            }
            return Ok((json!({"stopped":true}), 0));
        }
        Command::Uart { baud, send, secs } => {
            let port = d
                .uart()
                .ok_or_else(|| Failure::usage("device has no UART"))?;
            let (capture, mut rec) = dev(Capture::start(d, &[] as &[&str], false, Some(baud)))?;
            if let Some(s) = send {
                dev(port.write(s.as_bytes()))?
            }
            let deadline = Instant::now() + Duration::from_secs(secs);
            while Instant::now() < deadline && capture.pump(&mut rec, Duration::from_millis(100)) {}
            dev(capture.stop(d, &mut rec))?;
            let bytes: Vec<u8> = rec
                .bytes(port.rx(), 0.0, rec.duration())
                .into_iter()
                .flat_map(|c| c.bytes)
                .collect();
            if !cli.json {
                io::stdout()
                    .write_all(&bytes)
                    .map_err(|e| Failure::io(e.to_string()))?;
            }
            json!({"rx_bytes":bytes})
        }
        Command::Gpo { n, state } => {
            dev(gpio(d)?.set_output(n, parse_bool(&state)?))?;
            json!({"gpo":n,"state":state})
        }
        Command::Convert { .. } | Command::Stats { .. } | Command::Devices => unreachable!(),
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
        let (channels, blocks) =
            otii_arc::adapter::replay(include_str!("../../../fixtures/session1.hex"));
        let selected: Vec<_> = channels
            .into_iter()
            .filter(|c| c.name == "mc" || c.name == "mv")
            .collect();
        let mut rec = Recording::new(&selected);
        for b in &blocks {
            rec.push(b)
        }
        let base =
            std::env::temp_dir().join(format!("benchwatt-cli-fixture-{}", std::process::id()));
        let native = base.with_extension("bwrec");
        let csv = base.with_extension("csv");
        rec.save(&native, &Meta::default()).unwrap();
        let (converted, code) = run(Cli {
            device: None,
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
            device: None,
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
