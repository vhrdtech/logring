---
level: basic
---

# The SD card

The [[drive]] in the field logs to a Secure Digital (SD) card of 32 GB. A technician takes the card out after a fault,
puts it into a laptop and copies the log off it. The laptop must see an ordinary card with files on it, while the
drive writes the log at the speed of raw sectors. This chapter shows how the drive sets the card up and why the ring's
[[segments|segment]] match the card's [[allocation unit]]. It also shows how the drive notices what a laptop did.

## The ring as a file

The card carries an [[exFAT]] volume, the file system of SD cards over 32 GB, which the drive uses on smaller cards too.
The ring lives in one file of it, `logring.wlr`, the [[ring file]]. The drive creates the file once, at its full size
and in one piece, and then writes the ring into its sectors directly. It never touches the file system while it logs, so
a power cut cannot damage the file system.

The rest of the card stays an ordinary volume. It holds a `README.txt` that says what the ring file is, and 128 MiB of
free room for files the drive or a person puts there. The field `rw_bytes` of [`FormatOptions`] sets that room, and
the ring file gets the rest in whole segments. A laptop reads the ring file like any other file: the `logring` tool
and the Python package read it from the mounted card.

<figure class="ww-figure" markdown="0">
--8<-- "docs/plots/card_layout.svg"
<figcaption>The drive's 32 GB card after the format: the ring file takes nearly all of it. The file starts with the
superblock's segment, then the meta segments, then the data ring. Drawn by <code>logring_figures</code> from a format
of a simulated card that keeps only the sectors written to it.</figcaption>
</figure>

On the drive's card the ring file starts at byte 146 800 640, 140 MiB into the card, and is 30 376 MiB long. Its
first segment holds the [[superblock]] twice, in slot A at its start and in slot B half a segment in. The writer
writes the two slots in turn, so a power cut during one leaves the other. The [[meta area]] follows with 4 segments,
then the [[data ring]] with 7589 segments.

## Segments match the allocation unit

An SD card does not write single sectors in place: its controller erases and programs flash in large units. The SD
specification calls such a unit the [[allocation unit]] and lets the card report its size. The speed a card's class
promises holds for a host that fills one allocation unit from start to end before it moves to the next.

logring therefore makes a segment the allocation unit, or a multiple of it when the unit is under 4 MiB. It also
starts the ring file on an allocation unit boundary. The writer fills each segment from its first block to its last, in
flushes, and only then opens the next one. The drive's card reports 4 MiB, so its segments are 4 MiB. A card that
reports nothing gets 4 MiB segments.

The segment size is fixed at the format and kept in the superblock. Each later mount reads it from there, so the
firmware never needs to know it.

## Mounting the card

At every boot the drive checks that the ring file is still where it put it, before it writes a single sector. A
laptop may have done anything to the card since: deleted the file, copied it back, defragmented the volume. Writing
by sector address into a file that moved would destroy other files. The drive's [`mount_sd`] handles every case:

```rs
--8<-- "crates/logring_example/src/sd.rs:mount_sd"
```

The function [`exfat::open`] mounts the volume, finds the ring file and compares it with the superblock's
[[binding]]. The binding holds the volume's serial number, the file's first cluster, its length and its first
sector. A ring file that matches is bound, and the drive mounts the ring in it as on a raw card. Anything else is
unbound, and [`exfat::Unbound`] says why:

| Reason | What the drive finds |
|---|---|
| `Volume` | no exFAT volume it accepts: a camera's card, a damaged volume |
| `Missing` | no `logring.wlr` in the root directory: deleted, renamed or moved into a folder |
| `NotAFile` | `logring.wlr` is a directory |
| `NotContiguous` | the file has a cluster chain: a laptop rewrote or defragmented it |
| `Size` | the file is empty, not whole sectors, or its valid length differs from its length |
| `NotAllocated` | some of the file's clusters are free in the allocation bitmap |
| `NoHeader` | neither superblock slot decodes: another file took the place |
| `OtherCard` | the binding names another volume: the file came from another card |
| `Moved` | the file is not where the binding says: copied elsewhere, or replaced by a copy |
| `Resized` | the file's length differs from the binding's |
| `Geometry` | the superblock's layout does not fit the file |

A file that a laptop cut short or extended usually shows as `Size`. `Resized` catches one whose lengths still agree
with each other but not with the binding.

The drive answers an unbound ring file by making it anew with [`recreate_ring`]. That loses the log in it but keeps
every other file. A card without a usable volume gets a full format with [`format_card`]. A new card in the drive is
such a card: its first boot formats it, and its second boot finds the ring file bound.

## The card check

A format starts with a check of the card, because some cards lie. A fake card reports 32 GB but holds far less, and
its writes past the real size wrap round onto earlier sectors. The aliasing test writes marks across the whole card
and reads them back. The format thus refuses such a card before a single record goes to it.

The second part of the check measures how fast the card writes flushes of every power of two from 16 KiB to 1 MiB. The
smallest size within 90 % of the fastest becomes the card's `flush_bytes`, kept in the superblock for every later mount.
[The format reference](format.md#card-check) gives the exact steps. The drive example's simulated card takes no time to
write, so the example turns this measurement off and keeps the drive's own 16 KiB.

## How long the card holds

A minute of the drive's bench takes 1033 KiB of the card's data ring. The 7589 segments of 4 MiB therefore hold the
last 20.9 days of the bench. After that the ring goes round, and [When the ring is full](ring_full.md) says what
happens then.
