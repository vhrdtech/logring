---
level: basic
---

# Flushes and power cuts

The [[drive]] has no battery for its logger. When someone pulls the bench's plug, the power goes at once, in the
middle of whatever the firmware was doing. This chapter answers what such a power cut costs and how the firmware
chooses that cost. The answer lies in the [[flush]]: what it writes, when it writes, and what the next boot finds
when one stops halfway.

## What a flush writes

The records of all tracks wait in the [[stage buffer]] in RAM until a flush writes them to the card. A flush is one
write of whole [[blocks|block]] of 512 bytes. It starts with a [[flush head]], which says where every track's
[[positions|position]] stand, then come the records. Padding fills the rest of its last block, so the next flush
starts on a block of its own.

Every block ends with an 8-byte [[trailer]]. Its tag says that the block belongs to this ring and this [[segment]]. Its
distance leads back to the first block of its flush, and a cyclic redundancy check (CRC) covers the whole block. The 504 bytes before the trailer carry the flush's head and records.

<figure class="ww-figure" markdown="0">
--8<-- "docs/plots/flush.svg"
<figcaption>The first data segment of a ring that the drive wrote for four seconds, block by block. Each flush ends
with padding to the end of its last block, and every block ends in its trailer. Drawn by
<code>logring_figures</code> from the drive example, with segments of 8 KiB and flushes of 1 KiB instead of 64 KiB and
16 KiB, so that a segment fits the page.</figcaption>
</figure>

A flush thus decodes on its own: its head gives the positions, and the records follow. Nothing before or after it
needs to be read. This is what lets a reader start anywhere, and lets the next mount trust everything before the last
complete flush.

## When the writer flushes

The writer flushes in four cases, whichever comes first:

- the flush has grown to `flush_bytes` of the [`Config`], 16 KiB on the drive;
- the oldest record has waited `flush_interval_ms`, 1000 ms by default, and the firmware calls the tick;
- the firmware calls [`Logring::sync`], before it powers down on purpose, say;
- a record of an [[Immediate|durability]] track arrives.

The first two set what a power cut can lose. A larger flush writes less padding and fewer flush heads, so the card
holds more data. A smaller flush loses less when the power goes, because less waits in RAM. The interval caps the
wait on a quiet ring, where a flush would take minutes to fill.

The table shows the drive's first minute with four flush sizes. Each row is a run of the drive example on the
simulated card, as the example's `story` prints it:

| `flush_bytes` | Flushes | By the tick | Data on the card | Of it padding |
|---|---|---|---|---|
| 2 KiB | 516 | 0 | 1261 KiB | 225 KiB |
| 8 KiB | 138 | 0 | 1065 KiB | 44 KiB |
| 16 KiB | 70 | 0 | 1032 KiB | 13 KiB |
| 32 KiB | 69 | 47 | 1039 KiB | 21 KiB |

Up to 16 KiB, every flush fills before a second passes, so the size alone decides. At 32 KiB the second runs out first
for most flushes, and the tick writes them half full. A flush of 32 KiB therefore costs as much as one of 16 KiB here,
and saves nothing. The drive takes 16 KiB: the largest size that the drive's data fills within the interval.

[`Logring::set_flush`] changes both values while the firmware runs. A drive could flush more often while a test runs,
and less often while it idles.

## Immediate tracks

Some records are worth a flush of their own. The drive's mode changes a few times an hour, and a reader of a fault
wants to know the mode right before it. The `mode` track is therefore Immediate: its record goes to the card in a
flush before [`Drive::mode`] returns, together with whatever else waited in the stage buffer.

An Immediate record costs the padding of a flush, almost a block, and a write to the card. That suits a track with a
few records a minute, not the `can0` track with a thousand a second. The [[system track]] is always Immediate, so a
session start, a recording and a marker are on the card when their call returns.

## What a power cut costs

The drive example cuts the power during the first minute. Its simulated card stops after it has written 655 560
bytes, in the middle of a block, 37 seconds into the run. The block it was writing holds a damaged byte, as a block
does when the power fails while the card programs it.

<figure class="ww-figure" markdown="0">
--8<-- "docs/plots/power_cut.svg"
<figcaption>The segment the power cut tore, and the segment the next mount opened. The flush being written lost its
torn block and what came after it; everything before stayed. Drawn by <code>logring_figures</code> from the drive
example's power cut.</figcaption>
</figure>

The firmware had pushed 37 046 frames when the power went. After the next mount the card holds 37 016 of them, so the
cut cost 30 frames. Both changes of mode are on the card, since the `mode` track is Immediate. The next mount counted
1 torn block and 1 torn record and wrote both counts into the new session's first record.

The loss is small here because the power failed while a flush was on its way: the blocks of it that reached the card
count. In the worst case a cut loses the whole stage buffer: up to `flush_bytes` of records or a flush interval of
data, whichever is smaller. A cut never costs what a completed write put on the card. The writer never writes a
block twice in one lap of the ring. The call [`Logring::durable`] tells the firmware how far a track's
records are safe on the card.

## How the next mount finds the end

The next boot's [[mount]] does not search the card. It reads the [[superblock]] for a hint of the newest segment and
checks a few segments past it. Inside the newest segment it finds the last good block with a binary search. That
works because the written blocks form a prefix of the segment, and each block's tag and CRC say whether it belongs.

From the last good block, the trailer's distance leads back to the start of its flush. That flush's head gives every
track's position, and the mount reads the flush's records to the end. Where a record breaks off, the mount counts a
torn record. A block whose tag fits but whose CRC fails is a [[torn block]].

The mount then starts the new session in the next segment, whose seq 10 follows the torn one's. It never writes into the torn
segment again until the ring comes round to it. The [format reference](format.md#8-mount-and-recovery) lists every step
of the mount and the bounds on what it reads.
