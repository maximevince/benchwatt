use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ValueKind {
    String,
    Bool,
    Int,
    Enum,
    Scaled,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PropertyValue {
    String(String),
    Bool(bool),
    Int(i32),
    Enum(i32),
    Number(f64),
    Raw(Vec<u8>),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Property {
    pub id: u32,
    pub name: &'static str,
    pub kind: ValueKind,
    pub scale: f64,
}

const PROPS: &[Property] = &[
    Property {
        id: 1,
        name: "name",
        kind: ValueKind::String,
        scale: 1.0,
    },
    Property {
        id: 4,
        name: "intres",
        kind: ValueKind::Scaled,
        scale: 1e-3,
    },
    Property {
        id: 5,
        name: "4wire",
        kind: ValueKind::Bool,
        scale: 1.0,
    },
    Property {
        id: 6,
        name: "mainsrccurrmode",
        kind: ValueKind::Enum,
        scale: 1.0,
    },
    Property {
        id: 7,
        name: "forceoff",
        kind: ValueKind::Int,
        scale: 1.0,
    },
    Property {
        id: 8,
        name: "highrange",
        kind: ValueKind::Bool,
        scale: 1.0,
    },
    Property {
        id: 9,
        name: "main",
        kind: ValueKind::Bool,
        scale: 1.0,
    },
    Property {
        id: 10,
        name: "pwrreg",
        kind: ValueKind::Enum,
        scale: 1.0,
    },
    Property {
        id: 11,
        name: "mainvolt",
        kind: ValueKind::Scaled,
        scale: 1e-6,
    },
    Property {
        id: 12,
        name: "maxcur",
        kind: ValueKind::Scaled,
        scale: 1e-3,
    },
    Property {
        id: 13,
        name: "maincur",
        kind: ValueKind::Scaled,
        scale: 1e-6,
    },
    Property {
        id: 30,
        name: "adcres",
        kind: ValueKind::Scaled,
        scale: 1e-6,
    },
    Property {
        id: 40,
        name: "uart",
        kind: ValueKind::Bool,
        scale: 1.0,
    },
    Property {
        id: 41,
        name: "baudrate",
        kind: ValueKind::Int,
        scale: 1.0,
    },
    Property {
        id: 50,
        name: "gpiocfg",
        kind: ValueKind::Int,
        scale: 1.0,
    },
    Property {
        id: 51,
        name: "expvolt",
        kind: ValueKind::Scaled,
        scale: 1e-6,
    },
    Property {
        id: 52,
        name: "expoutvolt",
        kind: ValueKind::Scaled,
        scale: 1e-6,
    },
    Property {
        id: 128,
        name: "hwversion",
        kind: ValueKind::String,
        scale: 1.0,
    },
    Property {
        id: 129,
        name: "fwversion",
        kind: ValueKind::String,
        scale: 1.0,
    },
    Property {
        id: 130,
        name: "uid",
        kind: ValueKind::String,
        scale: 1.0,
    },
    Property {
        id: 0x80b,
        name: "mainvolt_max",
        kind: ValueKind::Scaled,
        scale: 1e-6,
    },
    Property {
        id: 141,
        name: "chinfo",
        kind: ValueKind::Int,
        scale: 1.0,
    },
];

pub fn property(id: u32) -> Option<Property> {
    PROPS.iter().find(|p| p.id == id).copied()
}
pub fn property_by_name(name: &str) -> Option<Property> {
    PROPS
        .iter()
        .find(|p| p.name.eq_ignore_ascii_case(name))
        .copied()
}
pub fn decode(id: u32, bytes: &[u8]) -> PropertyValue {
    let p = property(id);
    if p.map(|x| x.kind) == Some(ValueKind::String) {
        return PropertyValue::String(String::from_utf8_lossy(bytes).into_owned());
    }
    if p.map(|x| x.kind) == Some(ValueKind::Bool) {
        return PropertyValue::Bool(if bytes.len() >= 4 {
            u32::from_le_bytes(bytes[..4].try_into().unwrap()) != 0
        } else {
            bytes.first().copied().unwrap_or(0) != 0
        });
    }
    if bytes.len() >= 4 {
        let raw = i32::from_le_bytes(bytes[..4].try_into().unwrap());
        return match p.map(|x| x.kind) {
            Some(ValueKind::Scaled) => PropertyValue::Number(raw as f64 * p.unwrap().scale),
            Some(ValueKind::Enum) => PropertyValue::Enum(raw),
            _ => PropertyValue::Int(raw),
        };
    }
    PropertyValue::Raw(bytes.to_vec())
}
pub fn encode_set(id: u32, value: &PropertyValue) -> Result<Vec<u8>, String> {
    if matches!(id, 0x51..=0x60 | 0xB1 | 0xB5..=0xBB) {
        return Err(format!("property 0x{id:x} is calibration-only"));
    }
    let p = property(id).ok_or_else(|| format!("unknown property 0x{id:x}"))?;
    let mut out = id.to_le_bytes().to_vec();
    match (p.kind, value) {
        (ValueKind::String, PropertyValue::String(s)) => out.extend_from_slice(s.as_bytes()),
        (ValueKind::Bool, PropertyValue::Bool(v)) => {
            out.extend_from_slice(&u32::from(*v).to_le_bytes())
        }
        (ValueKind::Int, PropertyValue::Int(v)) | (ValueKind::Enum, PropertyValue::Enum(v)) => {
            out.extend_from_slice(&v.to_le_bytes())
        }
        (ValueKind::Scaled, PropertyValue::Number(v)) => {
            let raw = *v / p.scale;
            if !raw.is_finite() || raw.round() < i32::MIN as f64 || raw.round() > i32::MAX as f64 {
                return Err(format!("value out of range for {}", p.name));
            }
            out.extend_from_slice(&(raw.round() as i32).to_le_bytes());
        }
        _ => return Err(format!("wrong value type for {}", p.name)),
    }
    Ok(out)
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChannelInfo {
    pub ch: u16,
    pub kind: ChannelKind,
    pub flags: u8,
    pub rate_hz: u32,
    pub scale: f32,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelKind {
    Samples,
    Bytes,
    Digital,
}
#[derive(Debug, Clone, Default)]
pub struct ChannelTable(pub std::collections::HashMap<u16, ChannelInfo>);
impl ChannelTable {
    pub fn decode(bytes: &[u8]) -> Self {
        let mut t = Self::default();
        for x in bytes.chunks_exact(16) {
            let ch = u16::from_le_bytes([x[0], x[1]]);
            let kind = match x[2] {
                1 => ChannelKind::Bytes,
                2 => ChannelKind::Digital,
                _ => ChannelKind::Samples,
            };
            t.0.insert(
                ch,
                ChannelInfo {
                    ch,
                    kind,
                    flags: x[3],
                    rate_hz: u32::from_le_bytes(x[8..12].try_into().unwrap()),
                    scale: f32::from_le_bytes(x[12..16].try_into().unwrap()),
                },
            );
        }
        t
    }
    pub fn get(&self, ch: u16) -> ChannelInfo {
        self.0.get(&ch).copied().unwrap_or(ChannelInfo {
            ch,
            kind: ChannelKind::Samples,
            flags: 0x80,
            rate_hz: 1000,
            scale: 1.0,
        })
    }
}
impl fmt::Display for PropertyValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::String(v) => write!(f, "{v}"),
            Self::Bool(v) => write!(f, "{v}"),
            Self::Int(v) | Self::Enum(v) => write!(f, "{v}"),
            Self::Number(v) => write!(f, "{v}"),
            Self::Raw(v) => write!(f, "{:x?}", v),
        }
    }
}

/// Known channels: (id, short name, unit). Units for sn/sp/tm are not confirmed.
pub const CHANNELS: &[(u16, &str, &str)] = &[
    (0x00, "mc", "A"),
    (0x01, "mv", "V"),
    (0x02, "ac", "A"),
    (0x03, "av", "V"),
    (0x04, "sn", "V"),
    (0x05, "sp", "V"),
    (0x06, "mp", "W"),
    (0x07, "ap", "W"),
    (0x0c, "mc1", "A"),
    (0x0d, "mc2", "A"),
    (0x0e, "mc3", "A"),
    (0x10, "vb", "V"),
    (0x11, "vj", "V"),
    (0x12, "vpoe", "V"),
    (0x13, "vpwr", "V"),
    (0x14, "tp", "°C"),
    (0x15, "tm", ""),
    (0x16, "gpi", ""),
    (0x17, "rx", ""),
    (0x19, "tx", ""),
];

pub fn channel_name(ch: u16) -> &'static str {
    CHANNELS.iter().find(|c| c.0 == ch).map_or("", |c| c.1)
}

pub fn channel_by_name(name: &str) -> Option<u16> {
    CHANNELS.iter().find(|c| c.1 == name).map(|c| c.0)
}
