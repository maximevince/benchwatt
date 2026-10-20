use clap::{Parser, Subcommand};
use otii_arc::{
    Arc, Event,
    msg::BlockBody,
    prop::{self, PropertyValue, ValueKind},
};
use std::{
    collections::HashMap,
    fs::File,
    io::{BufWriter, Write},
    sync::{
        Arc as StdArc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

#[derive(Parser)]
#[command(name = "otii", version)]
struct Cli {
    #[arg(short, long)]
    port: Option<String>,
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
        v: f64,
    },
    Ilim {
        a: f64,
    },
    Monitor,
    Record {
        #[arg(long)]
        ch: String,
        #[arg(long)]
        secs: u64,
        #[arg(short, long, default_value = "out.csv")]
        output: String,
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
fn main() -> Result<(), String> {
    let cli = Cli::parse();
    let port = cli
        .port
        .or_else(otii_arc::device::find_port)
        .ok_or("no Arc serial port found")?;
    let arc = Arc::open(&port).map_err(|e| format!("open: {e:?}"))?;
    match cli.command {
        Command::Info => print_info(&arc),
        Command::Get { name } => {
            let id = parse_prop(&name)?;
            let value = arc.get_value(id).map_err(|e| format!("get: {e:?}"))?;
            println!(
                "{}: {value}",
                prop::property(id).map(|p| p.name).unwrap_or("unknown")
            );
        }
        Command::Set { name, value } => {
            let id = parse_prop(&name)?;
            let p = prop::property(id).ok_or_else(|| format!("unknown property 0x{id:x}"))?;
            let value = match p.kind {
                ValueKind::String => PropertyValue::String(value),
                ValueKind::Bool => PropertyValue::Bool(parse_bool(&value)?),
                ValueKind::Int => {
                    PropertyValue::Int(value.parse::<i32>().map_err(|e| e.to_string())?)
                }
                ValueKind::Enum => {
                    PropertyValue::Enum(value.parse::<i32>().map_err(|e| e.to_string())?)
                }
                ValueKind::Scaled => {
                    PropertyValue::Number(value.parse::<f64>().map_err(|e| e.to_string())?)
                }
            };
            arc.set(id, &value).map_err(|e| format!("set: {e:?}"))?;
        }
        Command::On => arc.output(true).map_err(|e| format!("on: {e:?}"))?,
        Command::Off => arc.output(false).map_err(|e| format!("off: {e:?}"))?,
        Command::Volt { v } => arc.set_voltage(v).map_err(|e| format!("volt: {e:?}"))?,
        Command::Ilim { a } => arc
            .set_current_limit(a)
            .map_err(|e| format!("ilim: {e:?}"))?,
        Command::Monitor => monitor(&arc)?,
        Command::Record { ch, secs, output } => record(&arc, &ch, secs, &output)?,
        Command::Uart { baud, send, secs } => uart(&arc, baud, send.as_deref(), secs)?,
        Command::Gpo { n, state } => arc
            .set_gpo(n, parse_bool(&state)?)
            .map_err(|e| format!("gpo: {e:?}"))?,
    }
    Ok(())
}
fn parse_bool(s: &str) -> Result<bool, String> {
    match s.to_ascii_lowercase().as_str() {
        "on" | "1" | "true" => Ok(true),
        "off" | "0" | "false" => Ok(false),
        _ => Err(format!("expected on/off/1/0, got {s}")),
    }
}
fn parse_prop(s: &str) -> Result<u32, String> {
    if let Some(x) = s.strip_prefix("0x") {
        u32::from_str_radix(x, 16).map_err(|e| e.to_string())
    } else {
        prop::property_by_name(s)
            .map(|p| p.id)
            .ok_or_else(|| format!("unknown property {s}"))
    }
}
fn print_info(arc: &Arc) {
    let info = arc.info();
    println!(
        "Arc protocol {}  max payload {}  device 0x{:08x}  features 0x{:x}",
        info.proto, info.max_len, info.devtype, info.featurebits
    );
    for id in [1, 0x80, 0x81, 0x82, 9, 11, 12, 6, 10, 0x33, 0x80b] {
        if let Some(value) = info.properties.get(&id) {
            println!(
                "{:<18} {}",
                prop::property(id).map(|p| p.name).unwrap_or("unknown"),
                value
            );
        }
    }
    println!("Channels:");
    let mut channels: Vec<_> = info.chinfo.0.values().collect();
    channels.sort_by_key(|c| c.ch);
    for c in channels {
        println!(
            "  0x{:02x} {:<8} {:?} {:>5} Hz flags 0x{:02x} scale {}",
            c.ch,
            channel_name(c.ch),
            c.kind,
            c.rate_hz,
            c.flags,
            c.scale
        );
    }
}
fn channel_name(ch: u16) -> &'static str {
    prop::channel_name(ch)
}
fn stop_flag() -> Result<StdArc<AtomicBool>, String> {
    let run = StdArc::new(AtomicBool::new(true));
    let flag = run.clone();
    ctrlc::set_handler(move || flag.store(false, Ordering::Relaxed)).map_err(|e| e.to_string())?;
    Ok(run)
}
fn monitor(arc: &Arc) -> Result<(), String> {
    let run = stop_flag()?;
    let rx = arc.events();
    let mut values = HashMap::<u32, f32>::new();
    let mut gpi = 0;
    let mut last = Instant::now();
    while run.load(Ordering::Relaxed) {
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(Event::Monitor { ch, value }) => {
                values.insert(ch, value);
            }
            Ok(Event::Gpi(mask)) => gpi = mask,
            Ok(Event::Alert { name, .. }) => eprintln!("alert: {name}"),
            _ => {}
        }
        if last.elapsed() >= Duration::from_millis(200) {
            println!(
                "mv {:>8.4} V  mc {:>9.6} A  vb {:>7.3} V  tp {:>6.2} C  gpi 0x{gpi:x}",
                values.get(&1).copied().unwrap_or(f32::NAN),
                values.get(&0).copied().unwrap_or(f32::NAN),
                values.get(&0x10).copied().unwrap_or(f32::NAN),
                values.get(&0x14).copied().unwrap_or(f32::NAN)
            );
            last = Instant::now();
        }
    }
    Ok(())
}
fn parse_channel(s: &str) -> Result<u32, String> {
    prop::channel_by_name(s)
        .map(u32::from)
        .ok_or_else(|| format!("unknown channel {s}"))
}
fn record(arc: &Arc, ch: &str, secs: u64, output: &str) -> Result<(), String> {
    let names: Vec<_> = ch.split(',').map(str::trim).collect();
    let channels: Vec<u32> = names
        .iter()
        .map(|s| parse_channel(s))
        .collect::<Result<_, _>>()?;
    let stem = output.strip_suffix(".csv").unwrap_or(output);
    let mut writers = HashMap::new();
    for (name, &id) in names.iter().zip(&channels) {
        let file = File::create(format!("{stem}_{name}.csv")).map_err(|e| e.to_string())?;
        let mut writer = BufWriter::new(file);
        writeln!(writer, "time_s,value").map_err(|e| e.to_string())?;
        writers.insert(id, writer);
    }
    let rx = arc.events();
    arc.start_recording(&channels)
        .map_err(|e| format!("record start: {e:?}"))?;
    let deadline = Instant::now() + Duration::from_secs(secs);
    let mut gaps = 0u64;
    let mut result = Ok(());
    while Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(Event::Data { ms, block }) => {
                if let Some(writer) = writers.get_mut(&(block.ch as u32)) {
                    let values: Vec<f64> = match block.body {
                        BlockBody::Samples(samples) => samples,
                        BlockBody::Digital(masks) => masks.into_iter().map(f64::from).collect(),
                        BlockBody::Bytes(bytes) => bytes.into_iter().map(f64::from).collect(),
                    };
                    let n = values.len();
                    for (i, value) in values.into_iter().enumerate() {
                        if let Err(e) = writeln!(
                            writer,
                            "{:.9},{value}",
                            ms as f64 / 1000.0 + i as f64 / n as f64 / 1000.0
                        ) {
                            result = Err(e.to_string());
                            break;
                        }
                    }
                }
            }
            Ok(Event::Gap { from, to }) => {
                gaps += u64::from(to.wrapping_sub(from).saturating_sub(1));
                eprintln!("gap: {from} -> {to}");
            }
            _ => {}
        }
        if result.is_err() {
            break;
        }
    }
    let stop = arc
        .stop_recording()
        .map_err(|e| format!("record stop: {e:?}"));
    for (_, mut writer) in writers {
        writer.flush().map_err(|e| e.to_string())?;
    }
    eprintln!("missing {gaps} data packets");
    result?;
    stop
}
fn uart(arc: &Arc, baud: u32, send: Option<&str>, secs: u64) -> Result<(), String> {
    let rx = arc.events();
    arc.uart_open(baud)
        .map_err(|e| format!("uart open: {e:?}"))?;
    if let Err(e) = arc.start_recording(&[0x17]) {
        let _ = arc.uart_close();
        return Err(format!("record start: {e:?}"));
    }
    let mut result = Ok(());
    if let Some(text) = send
        && let Err(e) = arc.uart_write(text.as_bytes())
    {
        result = Err(format!("uart write: {e:?}"));
    }
    let deadline = Instant::now() + Duration::from_secs(secs);
    while result.is_ok() && Instant::now() < deadline {
        if let Ok(Event::Data { block, .. }) = rx.recv_timeout(Duration::from_millis(100))
            && block.ch == 0x17
            && let BlockBody::Bytes(bytes) = block.body
        {
            if let Err(e) = std::io::stdout().write_all(&bytes) {
                result = Err(e.to_string());
            }
            let _ = std::io::stdout().flush();
        }
    }
    let stop = arc
        .stop_recording()
        .map_err(|e| format!("record stop: {e:?}"));
    let close = arc.uart_close().map_err(|e| format!("uart close: {e:?}"));
    result?;
    stop?;
    close
}
