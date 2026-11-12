//! Hardware smoke test: record from a real Arc into a `Recording` and print what came out.
//! Expects a ~100 Ω load on the main output and TX→RX, GPO1→GPI1 loopbacks.
use benchwatt_session::{Capture, rec::View};
use std::time::{Duration, Instant};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dev = benchwatt_session::open(&benchwatt_session::select(Some("arc"))?)?;
    let dev = dev.as_ref();
    let (supply, gpio, uart) = (
        dev.supply().ok_or("no supply")?,
        dev.gpio().ok_or("no GPIO")?,
        dev.uart().ok_or("no UART")?,
    );
    supply.set_voltage(1, 1.8)?;
    supply.set_current_limit(1, 0.05)?;
    supply.set_enabled(1, true)?;
    std::thread::sleep(Duration::from_millis(300));

    let (mut capture, mut rec) = Capture::start(dev, &["mc", "mv", "mp"], true, Some(115200))?;
    let start = Instant::now();
    let mut next_tick = Duration::ZERO;
    let mut tick = 0;
    while start.elapsed() < Duration::from_secs(3) {
        if start.elapsed() >= next_tick {
            gpio.set_output(1, tick % 2 == 0)?;
            uart.write(format!("tick {tick}\n").as_bytes())?;
            tick += 1;
            next_tick += Duration::from_millis(500);
        }
        capture.pump(&mut rec, Duration::from_millis(5));
    }
    capture.stop(dev, &mut rec)?;
    supply.set_enabled(1, false)?;
    gpio.set_output(1, false)?;

    let d = rec.duration();
    println!(
        "duration {d:.3} s, gaps {:?}, samples {:?}",
        rec.gaps(),
        rec.sample_counts()
    );
    for name in ["mc", "mv", "mp"] {
        let ch = benchwatt_session::channel(dev, name)?.id;
        if let Some(s) = rec.stats(ch, 0.0, d) {
            println!(
                "{name:>3}: mean {:.6} min {:.6} max {:.6} rms {:.6} n {}",
                s.mean, s.min, s.max, s.rms, s.count
            );
        }
    }
    println!(
        "charge {:?} mAh, energy {:?}",
        rec.charge_mah(0.0, d),
        rec.energy(0.0, d)
    );
    let mc = benchwatt_session::channel(dev, "mc")?.id;
    for px in [800, 20_000] {
        match rec.view(mc, 0.0, d, px) {
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
    let text: Vec<u8> = rec
        .bytes(uart.rx(), 0.0, d)
        .into_iter()
        .flat_map(|c| c.bytes)
        .collect();
    println!("uart rx: {:?}", String::from_utf8_lossy(&text));
    Ok(())
}
