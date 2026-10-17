//! The items of the system track (track 0 of the data ring) and of the meta area: hand-coded, evolvable
//! (a kind byte, then fields; readers ignore bytes a later version appends).

use crate::format::header::{MAX_TRACKS, TrackEntry, decode_time, encode_time};
use crate::format::{Dec, Enc};
use wire_weaver::shrink_wrap::{DeserializeShrinkWrap, SerializeShrinkWrap};
use wire_weaver::ww_version::FullVersion;
use ww_stream::Second;

/// Longest encoded [FullVersion] of the API root in a [MetaEvent::Session].
pub const MAX_API: usize = 128;

/// Longest string in a [SystemEvent] or [MetaEvent].
pub const MAX_LABEL: usize = 64;

/// What the recorder itself logs into track 0.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SystemEvent<'a> {
    /// The first record of every session.
    SessionStart {
        session: u32,
        /// What the mount found, see [Recovery](crate::Recovery).
        torn_blocks: u32,
        torn_records: u32,
        formatted: bool,
        device_id: &'a str,
        fw: &'a str,
        hw: &'a str,
        bundle_hash: u64,
    },
    /// A marker: a button press, `ww_recorder.marker()`.
    Marker {
        kind: u8,
        label: &'a str,
        value: Option<i64>,
    },
    RecordingStart {
        id: u32,
        trigger: u8,
    },
    RecordingStop {
        id: u32,
        reason: u8,
    },
    /// Items of a track the recorder refused (the ring full, drop newest).
    Dropped {
        track: u32,
        units: u64,
    },
    /// Something about the card: a write error, a stall over the threshold.
    CardEvent {
        kind: u8,
        value: u64,
    },
}

impl<'a> SystemEvent<'a> {
    pub fn encode(&self, enc: &mut Enc) -> Option<()> {
        match *self {
            SystemEvent::SessionStart {
                session,
                torn_blocks,
                torn_records,
                formatted,
                device_id,
                fw,
                hw,
                bundle_hash,
            } => {
                enc.u8(1)?;
                enc.u32le(session)?;
                enc.uvar(torn_blocks as u64)?;
                enc.uvar(torn_records as u64)?;
                enc.u8(formatted as u8)?;
                str_blob(enc, device_id)?;
                str_blob(enc, fw)?;
                str_blob(enc, hw)?;
                enc.u64le(bundle_hash)
            }
            SystemEvent::Marker { kind, label, value } => {
                enc.u8(2)?;
                enc.u8(kind)?;
                str_blob(enc, label)?;
                match value {
                    None => enc.u8(0),
                    Some(v) => {
                        enc.u8(1)?;
                        enc.ivar(v)
                    }
                }
            }
            SystemEvent::RecordingStart { id, trigger } => {
                enc.u8(3)?;
                enc.u32le(id)?;
                enc.u8(trigger)
            }
            SystemEvent::RecordingStop { id, reason } => {
                enc.u8(4)?;
                enc.u32le(id)?;
                enc.u8(reason)
            }
            SystemEvent::Dropped { track, units } => {
                enc.u8(5)?;
                enc.uvar(track as u64)?;
                enc.uvar(units)
            }
            SystemEvent::CardEvent { kind, value } => {
                enc.u8(6)?;
                enc.u8(kind)?;
                enc.uvar(value)
            }
        }
    }

    /// `None` on garbage or an unknown kind.
    pub fn decode(dec: &mut Dec<'a>) -> Option<Self> {
        Some(match dec.u8()? {
            1 => SystemEvent::SessionStart {
                session: dec.u32le()?,
                torn_blocks: u32::try_from(dec.uvar()?).ok()?,
                torn_records: u32::try_from(dec.uvar()?).ok()?,
                formatted: dec.u8()? != 0,
                device_id: dec.str()?,
                fw: dec.str()?,
                hw: dec.str()?,
                bundle_hash: dec.u64le()?,
            },
            2 => SystemEvent::Marker {
                kind: dec.u8()?,
                label: dec.str()?,
                value: match dec.u8()? {
                    0 => None,
                    1 => Some(dec.ivar()?),
                    _ => return None,
                },
            },
            3 => SystemEvent::RecordingStart {
                id: dec.u32le()?,
                trigger: dec.u8()?,
            },
            4 => SystemEvent::RecordingStop {
                id: dec.u32le()?,
                reason: dec.u8()?,
            },
            5 => SystemEvent::Dropped {
                track: u32::try_from(dec.uvar()?).ok()?,
                units: dec.uvar()?,
            },
            6 => SystemEvent::CardEvent {
                kind: dec.u8()?,
                value: dec.uvar()?,
            },
            _ => return None,
        })
    }
}

fn str_blob(enc: &mut Enc, s: &str) -> Option<()> {
    if s.len() > MAX_LABEL {
        return None;
    }
    enc.blob(s.as_bytes())
}

/// A record of the meta area (track 0 of the meta ring). A bundle's bytes follow its header in the record and are
/// not part of this enum: see [MetaEvent::Bundle].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MetaEvent<'a> {
    /// A session and its track table (the entries follow the header, `tracks` of them, each a [TrackEntry]).
    Session {
        session: u32,
        /// The API root's id ([api_root_id](crate::api_root_id)), part of every track's key.
        api_root: u64,
        /// The API root crate's name and version, as the firmware has them (`<ROOT_TRAIT>_FULL_GID`): a fallback
        /// for a host that doesn't have the bundle with `bundle_hash`. Tracks and records name nothing by string.
        api: FullVersion<'a>,
        device_id: &'a str,
        fw: &'a str,
        hw: &'a str,
        bundle_hash: u64,
        tracks: u32,
    },
    /// The API bundle with this hash: the record's item holds this header and then the bundle's bytes.
    Bundle { hash: u64, len: u32 },
    /// A recording started (`stopped == false`) or stopped, in `session`, in data segment `seq`, at `t`.
    Recording {
        id: u32,
        stopped: bool,
        /// Trigger kind at a start, reason at a stop.
        why: u8,
        session: u32,
        seq: u64,
        t: Option<Second>,
    },
}

impl<'a> MetaEvent<'a> {
    pub fn encode(&self, enc: &mut Enc) -> Option<()> {
        match *self {
            MetaEvent::Session {
                session,
                api_root,
                api,
                device_id,
                fw,
                hw,
                bundle_hash,
                tracks,
            } => {
                enc.u8(1)?;
                enc.u32le(session)?;
                enc.u64le(api_root)?;
                let mut tmp = [0u8; MAX_API];
                enc.blob(api.to_ww_bytes(&mut tmp).ok()?)?;
                str_blob(enc, device_id)?;
                str_blob(enc, fw)?;
                str_blob(enc, hw)?;
                enc.u64le(bundle_hash)?;
                if tracks as usize >= MAX_TRACKS {
                    return None;
                }
                enc.uvar(tracks as u64)
            }
            MetaEvent::Bundle { hash, len } => {
                enc.u8(2)?;
                enc.u64le(hash)?;
                enc.u32le(len)
            }
            MetaEvent::Recording {
                id,
                stopped,
                why,
                session,
                seq,
                t,
            } => {
                enc.u8(3)?;
                enc.u32le(id)?;
                enc.u8(stopped as u8)?;
                enc.u8(why)?;
                enc.u32le(session)?;
                enc.u64le(seq)?;
                encode_time(enc, t)
            }
        }
    }

    /// The header; a session's track entries and a bundle's bytes are left in `dec`.
    pub fn decode(dec: &mut Dec<'a>) -> Option<Self> {
        Some(match dec.u8()? {
            1 => MetaEvent::Session {
                session: dec.u32le()?,
                api_root: dec.u64le()?,
                api: FullVersion::from_ww_bytes(dec.blob()?).ok()?,
                device_id: dec.str()?,
                fw: dec.str()?,
                hw: dec.str()?,
                bundle_hash: dec.u64le()?,
                tracks: {
                    let n = u32::try_from(dec.uvar()?).ok()?;
                    if n as usize >= MAX_TRACKS {
                        return None;
                    }
                    n
                },
            },
            2 => MetaEvent::Bundle {
                hash: dec.u64le()?,
                len: dec.u32le()?,
            },
            3 => MetaEvent::Recording {
                id: dec.u32le()?,
                stopped: dec.u8()? != 0,
                why: dec.u8()?,
                session: dec.u32le()?,
                seq: dec.u64le()?,
                t: decode_time(dec)?,
            },
            _ => return None,
        })
    }

    /// The track entries after a [MetaEvent::Session] header.
    pub fn entries(dec: &mut Dec<'a>, tracks: u32) -> impl Iterator<Item = Option<TrackEntry<'a>>> {
        let mut d = dec.clone();
        (0..tracks).map(move |_| TrackEntry::decode(&mut d))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_round_trip() {
        let evs = [
            SystemEvent::SessionStart {
                session: 3,
                torn_blocks: 1,
                torn_records: 0,
                formatted: true,
                device_id: "rePort-0042",
                fw: "usb_io_fw 1.2.3",
                hw: "rev A",
                bundle_hash: 0xabcd,
            },
            SystemEvent::Marker {
                kind: 1,
                label: "button",
                value: Some(-5),
            },
            SystemEvent::Marker {
                kind: 0,
                label: "",
                value: None,
            },
            SystemEvent::RecordingStart { id: 7, trigger: 1 },
            SystemEvent::RecordingStop { id: 7, reason: 2 },
            SystemEvent::Dropped {
                track: 300,
                units: 1 << 40,
            },
            SystemEvent::CardEvent {
                kind: 9,
                value: 250,
            },
        ];
        let mut buf = [0u8; 512];
        let mut enc = Enc::new(&mut buf);
        for e in &evs {
            e.encode(&mut enc).unwrap();
        }
        let n = enc.len();
        let mut dec = Dec::new(&buf[..n]);
        for e in &evs {
            assert_eq!(SystemEvent::decode(&mut dec).as_ref(), Some(e));
        }
        assert_eq!(dec.left(), 0);
        assert_eq!(SystemEvent::decode(&mut Dec::new(&[9])), None);
        for cut in 0..n {
            let _ = SystemEvent::decode(&mut Dec::new(&buf[..cut]));
        }
    }

    #[test]
    fn meta_round_trip() {
        let evs = [
            MetaEvent::Session {
                session: 1,
                api_root: 0x1234_5678_9abc,
                api: FullVersion::new(
                    "usb_io_fw",
                    wire_weaver::ww_version::Version::full(1, 2, 3, Some("rc.1"), None),
                ),
                device_id: "d",
                fw: "f",
                hw: "h",
                bundle_hash: 5,
                tracks: 0,
            },
            MetaEvent::Bundle { hash: 5, len: 100 },
            MetaEvent::Recording {
                id: 2,
                stopped: true,
                why: 3,
                session: 1,
                seq: 99,
                t: None,
            },
        ];
        let mut buf = [0u8; 256];
        let mut enc = Enc::new(&mut buf);
        for e in &evs {
            e.encode(&mut enc).unwrap();
        }
        let n = enc.len();
        let mut dec = Dec::new(&buf[..n]);
        for e in &evs {
            assert_eq!(MetaEvent::decode(&mut dec).as_ref(), Some(e));
        }
        assert_eq!(dec.left(), 0);
    }
}
