//! Hardware probe for the app's GPO and UART flows (TX→RX, GPO1→GPI1 loopbacks).
use otii_arc::{Arc, Event, PropertyValue, device::find_port};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

fn drain(events: &Receiver<Event>, ms: u64) -> (Option<u32>, usize, Vec<u8>) {
    let (mut gpi, mut data, mut rx) = (None, 0, Vec::new());
    let end = Instant::now() + Duration::from_millis(ms);
    while let Some(left) = end.checked_duration_since(Instant::now()) {
        match events.recv_timeout(left) {
            Ok(Event::Gpi(mask)) => gpi = Some(mask),
            Ok(Event::Data { block, .. }) => {
                data += 1;
                if let (0x17, otii_arc::msg::BlockBody::Bytes(b)) = (block.ch, &block.body) {
                    rx.extend_from_slice(b);
                }
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }
    (gpi, data, rx)
}

fn gpiocfg(arc: &Arc) -> Option<PropertyValue> {
    arc.get_value(0x32).ok()
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arc = Arc::open(&find_port().ok_or("no Arc found")?)?;
    let events = arc.events();

    println!("== GPO");
    println!("gpiocfg at start: {:?}", gpiocfg(&arc));
    for (n, on) in [(1, true), (2, true), (1, false), (2, false)] {
        arc.set_gpo(n, on)?;
        let (gpi, ..) = drain(&events, 400);
        println!(
            "gpo{n}={on}: gpiocfg {:?}, monitor gpi {gpi:?}",
            gpiocfg(&arc)
        );
    }

    println!("== A: record [mc], open UART mid-recording");
    arc.start_recording(&[0x00])?;
    drain(&events, 300);
    arc.uart_open(115200)?;
    arc.uart_write(b"hello A\n")?;
    let (_, data, rx) = drain(&events, 600);
    println!("data blocks {data}, rx {:?}", String::from_utf8_lossy(&rx));
    arc.stop_recording()?;
    arc.uart_close()?;

    println!("== B: rx selected, UART not opened");
    arc.start_recording(&[0x00, 0x17])?;
    arc.uart_write(b"hello B\n")
        .map_err(|e| println!("write: {e}"))
        .ok();
    let (_, _, rx) = drain(&events, 600);
    println!("rx {:?}", String::from_utf8_lossy(&rx));
    arc.stop_recording()?;

    println!("== C: UART opened before recording [mc, rx]");
    arc.uart_open(115200)?;
    arc.start_recording(&[0x00, 0x17])?;
    drain(&events, 200);
    arc.uart_write(b"hello C\n")?;
    let (_, _, rx) = drain(&events, 600);
    println!("rx {:?}", String::from_utf8_lossy(&rx));
    arc.stop_recording()?;
    arc.uart_close()?;

    println!("== D: record [mc, mv, mp, rx] with UART closed, then open UART mid-recording");
    arc.start_recording(&[0x00, 0x01, 0x06, 0x17])?;
    drain(&events, 300);
    arc.uart_open(115200)?;
    arc.uart_write(b"hello D\n")?;
    let (_, _, rx) = drain(&events, 600);
    println!("rx {:?}", String::from_utf8_lossy(&rx));
    arc.stop_recording()?;
    arc.uart_close()?;
    Ok(())
}
