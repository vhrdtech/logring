//! Markers and recordings, derived from the system track's events ([decode::system_events]): the `logring`
//! CLI's `info` and `logring_py`'s `markers`/`recordings` both want them as plain, owned lists rather than
//! walking [logring::system::SystemEvent] themselves.

use crate::open::LogFile;
use logring::system::SystemEvent;

/// A marker (`ww_recorder.marker()`), [SystemEvent::Marker].
#[derive(Debug, Clone)]
pub struct Marker {
    pub pos: u64,
    pub t_ps: Option<i128>,
    pub kind: u8,
    pub label: String,
    pub value: Option<i64>,
}

/// A recording, [SystemEvent::RecordingStart]/[SystemEvent::RecordingStop] paired by id (at most one runs at a
/// time, as the device writes them).
#[derive(Debug, Clone)]
pub struct Recording {
    pub id: u32,
    pub trigger: u8,
    pub start_pos: u64,
    pub start_t_ps: Option<i128>,
    /// `None` while still running (or when its stop was lost to a torn write).
    pub stop_pos: Option<u64>,
    pub stop_t_ps: Option<i128>,
    pub stop_reason: Option<u8>,
}

fn t_ps(head: &ww_stream::StreamHead) -> Option<i128> {
    head.t.as_ref().and_then(logring::reader::time_ps)
}

/// Every marker in file order.
pub fn markers(log: &LogFile) -> Vec<Marker> {
    system_events_with_head(log)
        .filter_map(|(head, ev)| match ev {
            SystemEvent::Marker { kind, label, value } => Some(Marker {
                pos: head.pos,
                t_ps: t_ps(&head),
                kind,
                label: label.to_string(),
                value,
            }),
            _ => None,
        })
        .collect()
}

/// Every recording, started and (when its stop was seen) stopped, in file order.
pub fn recordings(log: &LogFile) -> Vec<Recording> {
    let mut out = Vec::new();
    let mut open: Option<Recording> = None;
    for (head, ev) in system_events_with_head(log) {
        match ev {
            SystemEvent::RecordingStart { id, trigger } => {
                if let Some(r) = open.take() {
                    out.push(r);
                }
                open = Some(Recording {
                    id,
                    trigger,
                    start_pos: head.pos,
                    start_t_ps: t_ps(&head),
                    stop_pos: None,
                    stop_t_ps: None,
                    stop_reason: None,
                });
            }
            SystemEvent::RecordingStop { id, reason } => {
                if let Some(r) = &mut open
                    && r.id == id
                {
                    r.stop_pos = Some(head.pos);
                    r.stop_t_ps = t_ps(&head);
                    r.stop_reason = Some(reason);
                    out.push(open.take().unwrap());
                }
            }
            _ => {}
        }
    }
    if let Some(r) = open {
        out.push(r);
    }
    out
}

/// [decode::system_events], paired with the record's [ww_stream::StreamHead] (position and device time), which
/// the decoded [SystemEvent] itself doesn't carry.
fn system_events_with_head(
    log: &LogFile,
) -> impl Iterator<Item = (ww_stream::StreamHead, SystemEvent<'_>)> {
    log.scan.records().filter_map(|(_, rec)| {
        if rec.track != 0 {
            return None;
        }
        let mut dec = logring::format::Dec::new(&rec.item);
        SystemEvent::decode(&mut dec).map(|ev| (rec.head, ev))
    })
}
