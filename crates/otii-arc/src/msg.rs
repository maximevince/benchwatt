use crate::prop::{self, ChannelKind, PropertyValue};

pub const CMD_RESP: u32 = 0xFF99_030E;
pub const DATA: u32 = 0xFF2A_8369;
pub const UPDATE: u32 = 0xFF2F_F2CE;
pub const ALERT: u32 = 0xFFC5_93CA;

#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub ch: u16,
    pub n: u16,
    pub rate_hz: u32,
    pub body: BlockBody,
}
#[derive(Debug, Clone, PartialEq)]
pub enum BlockBody {
    Samples(Vec<f64>),
    Bytes(Vec<u8>),
    Digital(Vec<u32>),
}
#[derive(Debug, Clone, PartialEq)]
pub struct UpdateRecord {
    pub rtype: u16,
    pub id: u32,
    pub value: UpdateValue,
    pub raw: Vec<u8>,
}
#[derive(Debug, Clone, PartialEq)]
pub enum UpdateValue {
    Property(PropertyValue),
    Monitor(f32),
    Gpi(u32),
    FourWire(u32),
    Raw(Vec<u8>),
}
#[derive(Debug, Clone, PartialEq)]
pub enum Message {
    CmdResp {
        txn: u32,
        status: i32,
        body: Vec<u8>,
    },
    Data {
        ms: u32,
        blocks: Vec<Block>,
    },
    Update {
        records: Vec<UpdateRecord>,
    },
    Alert {
        ids: Vec<u16>,
    },
    Unknown(Vec<u8>),
}

fn take<'a>(b: &'a [u8], n: usize, at: &mut usize) -> Option<&'a [u8]> {
    let e = at.checked_add(n)?;
    let v = b.get(*at..e)?;
    *at = e;
    Some(v)
}
fn u16v(b: &[u8], at: &mut usize) -> Option<u16> {
    Some(u16::from_le_bytes(take(b, 2, at)?.try_into().ok()?))
}
fn u32v(b: &[u8], at: &mut usize) -> Option<u32> {
    Some(u32::from_le_bytes(take(b, 4, at)?.try_into().ok()?))
}
pub fn parse(payload: &[u8], channels: &ChannelTable) -> Result<Message, String> {
    if payload.len() < 4 {
        return Err("payload too short".into());
    };
    let typ = u32::from_le_bytes(payload[..4].try_into().unwrap());
    let mut at = 4;
    match typ {
        CMD_RESP => Ok(Message::CmdResp {
            txn: u32v(payload, &mut at).ok_or("response txn")?,
            status: i32::from_le_bytes(
                take(payload, 4, &mut at)
                    .ok_or("response status")?
                    .try_into()
                    .unwrap(),
            ),
            body: payload[at..].to_vec(),
        }),
        DATA => {
            let ms = u32v(payload, &mut at).ok_or("data counter")?;
            let mut blocks = Vec::new();
            while at < payload.len() {
                let ch = u16v(payload, &mut at).ok_or("block channel")?;
                let n = u16v(payload, &mut at).ok_or("block count")?;
                let rate = u32v(payload, &mut at).ok_or("block rate")?;
                let ci = channels.get(ch);
                let body = match ci.kind {
                    ChannelKind::Bytes => BlockBody::Bytes(
                        take(payload, n as usize, &mut at)
                            .ok_or("byte block")?
                            .to_vec(),
                    ),
                    ChannelKind::Digital => {
                        let mut v = Vec::with_capacity(n as usize);
                        for _ in 0..n {
                            v.push(u32v(payload, &mut at).ok_or("digital block")?)
                        }
                        BlockBody::Digital(v)
                    }
                    ChannelKind::Samples => {
                        let mut v = Vec::with_capacity(n as usize);
                        for _ in 0..n {
                            let raw = take(payload, 4, &mut at)
                                .ok_or("sample")?
                                .try_into()
                                .unwrap();
                            let x = if ci.flags & 0x80 != 0 {
                                f32::from_le_bytes(raw) as f64
                            } else {
                                i32::from_le_bytes(raw) as f64
                            };
                            v.push(if ci.flags & 0x40 != 0 {
                                x * ci.scale as f64
                            } else {
                                x
                            });
                        }
                        BlockBody::Samples(v)
                    }
                };
                blocks.push(Block {
                    ch,
                    n,
                    rate_hz: rate,
                    body,
                });
            }
            Ok(Message::Data { ms, blocks })
        }
        UPDATE => {
            let _ = u32v(payload, &mut at).ok_or("update reserved")?;
            let mut records = Vec::new();
            while at < payload.len() {
                let rtype = u16v(payload, &mut at).ok_or("record type")?;
                let len = u16v(payload, &mut at).ok_or("record len")? as usize;
                let id = u32v(payload, &mut at).ok_or("record id")?;
                let raw = take(payload, len.checked_sub(4).ok_or("record length")?, &mut at)
                    .ok_or("record body")?
                    .to_vec();
                let value = if rtype == 1 {
                    UpdateValue::Property(prop::decode(id, &raw))
                } else if rtype == 2 && id == 0x16 {
                    UpdateValue::Gpi(u32::from_le_bytes(
                        raw.get(..4).ok_or("gpi")?.try_into().unwrap(),
                    ))
                } else if rtype == 2 && id == 0x10002 {
                    UpdateValue::FourWire(u32::from_le_bytes(
                        raw.get(..4).ok_or("4wire")?.try_into().unwrap(),
                    ))
                } else if rtype == 2 && id < 0x10000 && raw.len() >= 4 {
                    UpdateValue::Monitor(f32::from_le_bytes(raw[..4].try_into().unwrap()))
                } else {
                    UpdateValue::Raw(raw.clone())
                };
                records.push(UpdateRecord {
                    rtype,
                    id,
                    value,
                    raw,
                });
            }
            Ok(Message::Update { records })
        }
        ALERT => {
            let _ = u32v(payload, &mut at).ok_or("alert reserved")?;
            let mut ids = Vec::new();
            while at + 4 <= payload.len() {
                ids.push(u16v(payload, &mut at).unwrap());
                let len = u16v(payload, &mut at).unwrap() as usize;
                take(payload, len, &mut at).ok_or("alert body")?;
            }
            Ok(Message::Alert { ids })
        }
        _ => Ok(Message::Unknown(payload.to_vec())),
    }
}

pub use crate::prop::ChannelTable;
