---
level: basic
---

# Tracks, sessions and recordings

On the bench, the [[drive]] faults on overcurrent 12 seconds into a run. Someone resets it, and the next run ends as
planned. Weeks later an engineer reads the card and wants the frames of the run that faulted, and nothing else. To find
that run, the ring keeps [[tracks|track]] that stay the same across
boots and a [[session]] per boot. It also keeps the [[recordings|recording]] the firmware starts and stops, and
[[markers|marker]] at the moments that matter.

## A track and its path

A [[track]] is one stream of items of one type, such as the drive's `can0`. The firmware names a track by its path in
the device's API, a list of small numbers, and never by a string. The device has those numbers at hand, and a string
on every segment of the card would cost room for nothing.

The path alone could name a stream of another device. logring therefore hashes the API's name and major version
together with the path into the [[track key]]. The key stays the same across boots and across firmware updates that
keep the stream's path. A reader joins a track's records from all sessions by its key.

Within a session a track also has a number, its index in the session's [`TrackDesc`] list plus one. Track 0 is the
[[system track]], which logring writes itself. The number is short to store in every record, but it belongs to one
session: a firmware update may list the tracks in another order. The [[track table]] at the start of every
[[segment]] maps the numbers to keys and paths, so every segment decodes on its own. [The format
reference](format.md#5-track-table) lists the table's fields.

## Records and positions

Each push stores a [[record]]: the item, its track, its time and its [[position]]. The position counts the track's
items, from 0 for the first item the track ever had. A reader asks for "frames 12 000 to 13 000" by position and finds
them without reading the card from its start.

Positions go on across boots. At each [[mount]] logring reads the last [[flush]] on the card and continues every track
from there. In the example of this chapter, the first boot records 12 000 frames before the fault. The second boot's
first frame gets position 12 000, as if nothing had happened in between.

## Sessions

A [[session]] is everything one mount writes, from a boot to the next. Each mount starts its session in a new segment,
and the session's first record on the system track says which session it is. A reset, a crash and a power cut therefore
all end a session in the same way. A reader sees where each boot began without the firmware having to say so.

<figure class="ww-figure" markdown="0">
--8<-- "docs/figures/sessions_recordings.svg"
<figcaption>Two boots of the drive on one time line. Each boot is a session, each run a recording, and the fault has a
marker. A track's positions go on across the reset.</figcaption>
</figure>

The drive's first boot runs for 20 seconds and writes 4 segments. The second boot, 30 seconds long, writes 9 segments
after them. The sessions are numbered from 1 for the life of the card, so session 2 is the second boot since the card
was formatted.

## Recordings

A session is often too coarse. The drive boots once in the morning and runs many tests a day, and a reader wants one
of them. A [[recording]] marks such a stretch: the firmware starts it and stops it, each time with a reason of its own.

```rs
--8<-- "crates/logring_example/src/drive.rs:recording"
```

The drive starts a recording whenever the motor starts, with the trigger [`trigger::MOTOR_START`]. It stops the
recording when the motor stops as asked, with the reason [`reason::MOTOR_STOP`], or on a fault, with
[`reason::FAULT`]. The codes are the drive's own: logring stores them and a reader shows them.

[`Logring::recording_start`] returns the recording's id, one more than the last for the life of the card. Starting
a recording while one runs stops the running one first. logring keeps every start and stop in the system track and
in the [[meta area]], so a reader lists the recordings without reading the data.

## Markers

A [[marker]] is a labelled moment with an optional value, written to the card at once. The drive marks the overcurrent
fault with its kind [`marker::OVERCURRENT`], the label `phase a` and the phase current in mA as its value. A reader
lists markers like recordings and jumps to the moment.

A marker costs a [[flush]] of its own, because logring writes it at once. It suits rare events such as a button
press or a fault, not a value that changes every second. A value of that kind belongs in a track of its own.

## The system track

Track 0 holds what logring writes about the log itself: session starts, recordings and markers. Here is the system
track of the two boots, as the example reads it back from the card:

| Record | Fields |
|---|---|
| session start | session 1 |
| recording start | id 1, trigger 1 |
| marker | kind 1, value 41500 |
| recording stop | id 1, reason 2 |
| session start | session 2 |
| recording start | id 2, trigger 1 |
| recording stop | id 2, reason 1 |

A session start also carries what the mount found at the end of the last session. It counts the [[torn blocks|torn
block]] and the torn records there. [Flushes and power cuts](flushes.md) explains where those come from. [The format
reference](format.md#7-system-track-and-meta-records) lists every kind of system record with its fields.
