---
level: basic
---

# Reading a log on a PC

After this chapter you can read a card image with the `logring` tool and Python. You can export its tracks as CSV,
MCAP (a container format for robotics logs) or MF4 (Measurement Data Format version 4). You can also pick out one
recording or one stretch of time. The drive's card, back from the bench at a desk: how do you get at what it kept?

## Opening the ring

The `logring` tool and the Python package open the same sources. Each is a [[ring file]], a raw image, or a split
file. Each can also be a directory the operating system already mounted a card's volume under, with the ring file
found inside it. No root access is needed. `` [`logring_export::open`] `` tries them in that order, and the CLI and
Python share it.

```sh
logring info card.img
```

`logring info` lists the [[sessions]], the [[API bundles]] stored with them, and every [[track]]. For each track: its
key, its path in the device's API, how many records it holds, and whether its type is resolved.

## Resolved and unresolved tracks

A track's name and item fields come from the API bundle the session stored. This book's drive calls
`` [`Drive::mount`] `` with `` [`SessionInfo`] ``'s `bundle` left `None`, to keep the firmware side of the example
small. So `logring info` on its own card lists its four tracks by key and path rather than by name, each with every
record counted but none decoded.

Real firmware that stores its bundle gets names and decoded items for free. A session that saved `can0`'s bundle
lets `logring info` show `can0` by name. It lets `logring cat` print its frames as JSON, and `logring export` write
them to a file. A device without one still gets session and segment counts, record counts per track, and
`logring check`'s torn-block report. The ring never needs a bundle to be read for recovery, only to be read for
meaning.

## A card with a bundle

`logring make-image` builds a small ring with its own bundle, so export and `cat` have something to decode right
away:

```sh
logring make-image demo.wlr --tracks 2 --records 20
```
```text
demo.wlr (2 tracks, 20 records each, 524288 bytes)
```
```sh
logring info demo.wlr
```
```text
demo.wlr
  1 data segments, 1 meta segments, 1 sessions, 1 bundles
  session 1: make-image (logring_make_image 1.0.0 simulated), bundle 3a9ca14800d89da3
  bundle 3a9ca14800d89da3: 57 bytes, resolved
tracks:
  sensor_0                 key 3867600071426b85  path [1]  20 records, 0 decode error(s)  [resolved]
  sensor_1                 key d8620c08c7782816  path [2]  20 records, 0 decode error(s)  [resolved]
system track:
  0 marker(s), 0 recording(s) started, 0 stopped, 0 drop event(s), 0 card event(s)
```

Both tracks are `resolved`: the CLI decoded every record against the stored bundle.

<figure class="ww-figure" markdown="0">
--8<-- "docs/figures/export.svg"
<figcaption>The same ring exported three ways: CSV writes one file per track, MCAP one file with every resolved
track as a channel, MF4 plain numbers or, for a track that looks like a Controller Area Network (CAN) bus, ASAM bus
logging.</figcaption>
</figure>

```sh
logring export demo.wlr --track sensor_0 --format csv -o sensor_0.csv
```
```text
pos,t_ps,value
0,,1
1,,1.099833369255066
2,,1.1986693143844604
```

`export` without `--track` writes every resolved track: one file per track for CSV and MF4, one file with every
track as a channel for [[MCAP]]. `logring cat` prints one track's records as JSON lines instead of a file:

```sh
logring cat demo.wlr --track sensor_0
```
```text
{"pos":0,"t_ps":null,"value":1.0}
{"pos":1,"t_ps":null,"value":1.099833369255066}
```

## One recording or one time range

A track's rows carry the device time the firmware gave them. This demo's rows show `null`, since `make-image`'s
device never sets one; the drive's does, for every item.

`` [`events::recordings`] `` pairs a [[recording]]'s start and stop into its positions and times. `` [`events::markers`] ``
gives every [[marker]] the same way. A reader slices a track's rows between a recording's `start_pos` and
`stop_pos`, or between two times, without a dedicated flag.

## Python

```python
import logring

card = logring.open("demo.wlr")
df = card.track("sensor_0").to_pandas()
```

`to_pandas()` returns the same columns `export --format csv` writes, one row per record, built from `to_numpy()`
underneath. `card.recordings()` and `card.markers()` return the same fields as the Rust side, as plain dicts, for a
notebook to filter a `DataFrame` by.

Chapter 8 reads a device's ring the same way, without a card image: live, over its WireWeaver API.
