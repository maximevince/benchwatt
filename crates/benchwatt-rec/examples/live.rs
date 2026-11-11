//! Hardware smoke test: record from a real Arc into a `Recording` and print what came out.
//! Expects a ~100 Ω load on the main output and TX→RX, GPO1→GPI1 loopbacks.
use benchwatt_rec::{ChannelSpec, Kind, Recording, View};
use otii_arc::{Arc, Event, device::find_port, prop};
use std::time::{Duration, Instant};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let port = find_port().ok_or("no Arc found")?;
    let arc = Arc::open(&port)?;
    let table = arc.info().chinfo;
    let channels: [u16; 5] = [0x00, 0x01, 0x06, 0x16, 0x17];
    let specs: Vec<ChannelSpec> = channels
        .iter()
        .map(|&id| {
            let unit = prop::CHANNELS
                .iter()
                .find(|c| c.0 == id)
                .map_or("", |c| c.2);
            let kind = match id {
                0x16 => Kind::Digital,
                0x17 => Kind::Bytes,
                _ => Kind::Analog,
            };
            ChannelSpec {
                id,
                name: prop::channel_name(id).into(),
                unit: unit.into(),
                rate_hz: table.get(id).rate_hz,
                kind,
            }
        })
        .collect();
    let mut rec = Recording::new(&specs);

    arc.set_voltage(1.8)?;
    arc.set_current_limit(0.05)?;
    arc.output(true)?;
    arc.uart_open(115200)?;
    std::thread::sleep(Duration::from_millis(300));

    let events = arc.events();
    let ids: Vec<u32> = channels.iter().map(|&c| c as u32).collect();
    arc.start_recording(&ids)?;
    let start = Instant::now();
    let mut next_tick = Duration::ZERO;
    let mut tick = 0;
    while start.elapsed() < Duration::from_secs(3) {
        if start.elapsed() >= next_tick {
            arc.set_gpo(1, tick % 2 == 0)?;
            arc.uart_write(format!("tick {tick}\n").as_bytes())?;
            tick += 1;
            next_tick += Duration::from_millis(500);
        }
        if let Ok(Event::Data { ms, block }) = events.recv_timeout(Duration::from_millis(5)) {
            rec.push_block(ms, &block);
        }
    }
    arc.stop_recording()?;
    while let Ok(ev) = events.try_recv() {
        if let Event::Data { ms, block } = ev {
            rec.push_block(ms, &block);
        }
    }
    rec.stop();
    arc.output(false)?;
    arc.set_gpo(1, false)?;
    arc.uart_close()?;
    arc.set_voltage(3.3)?;
    arc.set_current_limit(3.0)?;

    let d = rec.duration();
    println!(
        "duration {d:.3} s, gaps {:?}, samples {:?}",
        rec.gaps(),
        rec.sample_counts()
    );
    for ch in [0x00u16, 0x01, 0x06] {
        if let Some(s) = rec.stats(ch, 0.0, d) {
            println!(
                "{:>3}: mean {:.6} min {:.6} max {:.6} rms {:.6} n {}",
                prop::channel_name(ch),
                s.mean,
                s.min,
                s.max,
                s.rms,
                s.count
            );
        }
    }
    println!(
        "charge {:?} mAh, energy {:?}",
        rec.charge_mah(0.0, d),
        rec.energy(0.0, d)
    );
    for px in [800, 20_000] {
        match rec.view(0x00, 0.0, d, px) {
            Some(View::Raw { t, .. }) => println!("view px={px}: raw {} points", t.len()),
            Some(View::Envelope { t, .. }) => {
                println!("view px={px}: envelope {} buckets", t.len())
            }
            None => println!("view px={px}: none"),
        }
    }
    let edges = rec.edges(0.0, d);
    println!(
        "gpi edges {}: {:?}",
        edges.len(),
        edges
            .iter()
            .map(|e| (format!("{:.3}", e.t), e.mask))
            .collect::<Vec<_>>()
    );
    let text: Vec<u8> = rec.uart(0.0, d).into_iter().flat_map(|c| c.bytes).collect();
    println!("uart rx: {:?}", String::from_utf8_lossy(&text));
    Ok(())
}
