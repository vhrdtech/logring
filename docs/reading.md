---
level: basic
---

# Reading a log on a PC

An engineer pulls the drive's card off the bench and sets it on a desk. It holds a session of `log`, `mode`,
`can0` and `vbat`, under one [[recording]] of however long the motor ran. How does the engineer get at what it
kept, one track or one stretch of time, on a PC?

## Opening the ring

The `logring` tool and the Python package open the same sources. Each is a [[ring file]], a raw image, or a split
file. Each can also be a directory the operating system already mounted a card's volume under, with the ring file
found inside it. No root access is needed.

`` [`logring_export::open`] `` resolves the path directly: a file as given, or `logring.wlr` found inside a
directory. The CLI and Python share it.

## The drive's card

`` [`Drive::mount`] `` stores the drive's own [[API bundle]] with the first session, and again once the meta ring
overwrote it. `build.rs` loads it from the same `trait Drive` the firmware calls, at build time, and the firmware
embeds the result. A reader needs nothing of the firmware's source to make sense of what the card kept.

```sh
logring info drive.wlr
```
```text
drive.wlr
  17 data segments, 1 meta segments, 1 sessions, 1 bundles
  session 1: drive-0042 (drive fw 1.4.0 drive rev C), bundle 1eb8ced7c6bdad9e
  bundle 1eb8ced7c6bdad9e: 797 bytes, resolved
tracks:
  log                      key 3aa950c141a0a925  path [0]  31 record(s), 0 decode error(s)  [resolved]
  can0                     key 7a9ea8d1ee0c2247  path [2]  60000 record(s), 0 decode error(s)  [resolved]
  mode                     key 9aaea4b8eb6aec94  path [1]  3 record(s), 0 decode error(s)  [resolved]
  vbat                     key daa3fcc997d665b6  path [3]  60 record(s), 0 decode error(s)  [resolved]
system track:
  0 marker(s), 1 recording(s) started, 1 stopped, 0 drop event(s), 0 card event(s)
```

Every track is named and resolved: the bundle lets `logring info` show `can0` by name, not only by key and path.

## Exporting

The same ring exports to CSV, MCAP or MF4.

<figure class="ww-figure" markdown="0">
--8<-- "docs/figures/export.svg"
<figcaption>The same ring exported three ways: CSV writes one file per track, MCAP one file with every resolved
track as a channel, MF4 plain numbers or, for a track that looks like a CAN bus, ASAM bus
logging.</figcaption>
</figure>

```sh
logring export drive.wlr --track can0 --format mf4 -o can0.mf4
```
```text
can0 -> can0.mf4
```

`can0`'s items look like a CAN frame: an `id` field and a byte payload. `export` then
writes bus logging as the ASAM defines it
for MF4, not a plain numeric channel. A tool such as CANape or asammdf reads the result as `CAN_DataFrame` frames
directly.

`export` without `--track` writes every resolved track at once. CSV and MF4 get one file per track; [[MCAP]] gets
one file with every track as a channel. `logring cat` prints one track's records as JSON lines instead of a file:

```sh
logring cat drive.wlr --track can0
```
```text
{"pos":0,"t_ps":0,"value":{"data":[184,11,0,0,0,0,0,0],"id":257}}
{"pos":1,"t_ps":1000000000,"value":{"data":[185,11,1,0,0,0,0,0],"id":257}}
```

Every row carries the drive's own device time, `t_ps`, picoseconds since boot. The drive sets it on every push.

## One recording or one time range

`` [`events::recordings`] `` pairs a recording's start and stop from the system track. Each gets one id, a trigger,
a reason, a [[position]] and a time. That position sits in the system track itself, 1 and 2 for this drive's
one-minute run, where the start and stop records live.

It is never a position in `can0`'s own stream, which holds 60 000 records of its own. Slicing `can0` at positions
1 and 2 would return its second and third frame, not the minute the motor ran. A reader slices a track by the
recording's times instead, `start_t_ps` and `stop_t_ps`, against each record's own device time.

## A card without a bundle

Not every card carries its own bundle: a card from older firmware, or a session logged before the firmware stored
one. Its tracks still hold every record. Only their meaning is missing, until a bundle for that API reaches the
reader some other way.

```sh
logring check nobundle.wlr
```
```text
nobundle.wlr
  warning track key.a8231e12f9543fab has no resolved type, 3 record(s) left undecoded
  clean
```

`check` warns about the unresolved track but still calls the card clean: nothing on it is corrupted, its records
are simply undecodable without more information. `logring info` lists the same track by its key rather than its
name, every record counted but none decoded. The ring never needs a bundle to be read for recovery, only to be
read for meaning.

## Python

```python
import logring

card = logring.open("drive.wlr")
rec = card.recordings()[0]
df = card.track("can0").to_pandas()
during = df[(df.t_ps >= rec["start_t_ps"]) & (df.t_ps <= rec["stop_t_ps"])]
```

`during` holds 59 000 of `can0`'s 60 000 rows. The recording starts a second into the run, once the motor is
actually turning, so the first second of frames falls outside it. `to_pandas()` returns `pos`, `t_ps` and one
column per numeric leaf field, built from `to_numpy()` underneath. A track with a string or enum field gets fewer
columns here than `export --format csv` writes for it, since those fields stay out of the numeric columns.
`card.recordings()` and `card.markers()` return the same fields as the Rust side, as plain dicts, for a notebook to
filter a `DataFrame` by.

[Reading over WireWeaver and USB](live_reading.md) reads a device's ring the same way, without a card image: live,
over its WireWeaver API.
