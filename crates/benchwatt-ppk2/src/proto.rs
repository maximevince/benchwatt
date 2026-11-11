//! The calibration reply, the sample word and the host-side spike filter.
use benchwatt_device::{Error, Result};

const R_DEFAULT: [f64; 5] = [1031.64, 101.65, 10.15, 0.94, 0.043];
/// Volts per ADC step after the ×4 scaling of the 14-bit value.
const ADC_STEP: f64 = 1.8 / 163840.0;

/// Per-range tables index by range 0 (lowest current) to 4.
#[derive(Clone, Debug, PartialEq)]
pub struct Calibration {
    pub r: [f64; 5],
    pub gs: [f64; 5],
    pub gi: [f64; 5],
    pub o: [f64; 5],
    pub s: [f64; 5],
    pub i: [f64; 5],
    pub ug: [f64; 5],
    pub vdd_mv: Option<u16>,
    pub hw: Option<String>,
    /// 1 ampere meter, 2 source meter.
    pub mode: Option<u8>,
    pub ia: Option<f64>,
    pub calibrated: Option<bool>,
}
impl Default for Calibration {
    fn default() -> Self {
        Self {
            r: R_DEFAULT,
            gs: [1.0; 5],
            gi: [1.0; 5],
            o: [0.0; 5],
            s: [0.0; 5],
            i: [0.0; 5],
            ug: [1.0; 5],
            vdd_mv: None,
            hw: None,
            mode: None,
            ia: None,
            calibrated: None,
        }
    }
}

/// Parse the reply to get-calibration. Table entries that are missing, not
/// finite or exactly zero keep their default; anything else, however small, is
/// used as sent.
pub fn parse_calibration(text: &str) -> Result<Calibration> {
    let mut cal = Calibration::default();
    let mut end = false;
    for line in text.lines().map(str::trim) {
        if line == "END" {
            end = true;
            break;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let (key, value) = (key.trim(), value.trim());
        let number = value.parse::<f64>().ok().filter(|x| x.is_finite());
        let index = key
            .chars()
            .last()
            .and_then(|c| c.to_digit(10))
            .filter(|&n| n < 5);
        if let Some(n) = index {
            let table = match &key[..key.len() - 1] {
                "R" => &mut cal.r,
                "GS" => &mut cal.gs,
                "GI" => &mut cal.gi,
                "O" => &mut cal.o,
                "S" => &mut cal.s,
                "I" => &mut cal.i,
                "UG" => &mut cal.ug,
                _ => continue,
            };
            if let Some(x) = number.filter(|&x| x != 0.0) {
                table[n as usize] = x;
            }
            continue;
        }
        match key {
            "VDD" => {
                cal.vdd_mv = number
                    .filter(|&x| x > 0.0 && x <= f64::from(u16::MAX))
                    .map(|x| x as u16)
            }
            "HW" => cal.hw = Some(value.into()),
            "mode" => cal.mode = number.filter(|&x| x == 1.0 || x == 2.0).map(|x| x as u8),
            "IA" => cal.ia = number,
            "Calibrated" => cal.calibrated = number.map(|x| x != 0.0),
            _ => {}
        }
    }
    if end {
        Ok(cal)
    } else {
        Err(Error::invalid("calibration reply has no END line"))
    }
}

/// One decoded sample word.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sample {
    pub amps: f64,
    pub range: u8,
    pub counter: u8,
    /// Digital inputs D0 (bit 0) to D7.
    pub bits: u8,
}

/// Convert one little-endian sample word; `vdd_mv` is the voltage last set.
pub fn convert(word: u32, cal: &Calibration, vdd_mv: f64) -> Sample {
    let range = ((word >> 14) & 7).min(4) as usize;
    let adc = f64::from(word & 0x3fff) * 4.0;
    let r = (adc - cal.o[range]) * ADC_STEP / cal.r[range];
    let amps = cal.ug[range]
        * (r * (cal.gs[range] * r + cal.gi[range]) + cal.s[range] * vdd_mv / 1000.0 + cal.i[range]);
    Sample {
        amps,
        range: range as u8,
        counter: ((word >> 18) & 0x3f) as u8,
        bits: (word >> 24) as u8,
    }
}

/// Samples lost between the expected counter value and the one received. A
/// loss of a multiple of 64 samples cannot be seen.
pub fn lost_samples(expected: u8, counter: u8) -> u8 {
    counter.wrapping_sub(expected) & 0x3f
}

/// Smooths the spike the PPK2 produces when it switches range: the three
/// samples from a range change on are replaced by a running average, and the
/// first two of them on range 4 are kept out of the averages altogether.
#[derive(Clone, Debug, Default)]
pub struct SpikeFilter {
    fast: f64,
    slow: f64,
    range: Option<u8>,
    left: u8,
}
impl SpikeFilter {
    pub fn reset(&mut self) {
        *self = Self::default();
    }
    pub fn apply(&mut self, amps: f64, range: u8) -> f64 {
        let Some(previous) = self.range.replace(range) else {
            (self.fast, self.slow) = (amps, amps);
            return amps;
        };
        if range != previous {
            self.left = 3;
        }
        let (fast, slow) = (self.fast, self.slow);
        self.fast += 0.18 * (amps - self.fast);
        self.slow += 0.06 * (amps - self.slow);
        if self.left == 0 {
            return amps;
        }
        if range == 4 && self.left > 1 {
            (self.fast, self.slow) = (fast, slow);
        }
        self.left -= 1;
        if range == 4 { self.slow } else { self.fast }
    }
}
