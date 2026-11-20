---
level: basic
---

# Flushes and power cuts

The [[drive]] has no battery for its logger. When someone pulls the bench's plug, the power goes at once, in the
middle of whatever the firmware was doing. This chapter answers what such a power cut costs and how the firmware
chooses that cost. The answer lies in the [[flush]]: what it writes, when it writes, and what the next boot finds
when one stops halfway.

## What a flush writes

A [[flush]] is what the writer puts on the card as one unit, in whole [[blocks|block]]. It starts with a [[flush
head]], which says where every track's [[positions|position]] stand, and the records follow. Padding fills the rest
of its last block, so the next flush starts on a block of its own.

Every block ends with a small [[trailer]]. Its tag says that the block belongs to this ring and this [[segment]]. Its
distance leads back to the first block of its flush, and a cyclic redundancy check (CRC) covers the whole block.
[The format reference](format.md#2-blocks) gives the trailer byte by byte.

<figure class="ww-figure" markdown="0">
--8<-- "docs/plots/flush.svg"
<figcaption>The first data segment of a ring that the drive wrote for four seconds, block by block. Each flush ends
with padding to the end of its last block, and every block ends in its trailer. Drawn by
<code>logring_figures</code> from the drive example, with segments of 8 KiB and flushes of 1 KiB instead of 64 KiB and
16 KiB, so that a segment fits the page.</figcaption>
</figure>

A flush thus decodes on its own: its head gives the positions, and the records follow. Nothing before or after it
needs to be read. A reader can therefore start at any flush, and the next mount keeps every block that reached the
card whole.

## The stage buffer

The writer builds each flush in the [[stage buffer]], the RAM the firmware lends it. The drive's stage buffer holds
4096 bytes, less than one of its flushes. Each time the stage buffer fills, the writer writes it to the card and goes
on with the same flush. A flush of 16 KiB thus reaches the card in four writes, and the last one ends the flush.

Only what sits in the stage buffer can be lost to a power cut. Everything that a completed write put on the card
stays there, because the writer never writes a block twice in one lap of the ring. The stage buffer's size is
therefore the first bound on what a cut costs.

## When a flush ends

The writer ends a flush in four cases, whichever comes first:

- the flush has grown to `flush_bytes` of the [`Config`], 16 KiB on the drive;
- the tick has seen the flush open for `flush_interval_ms`, 1000 ms by default;
- the firmware calls [`Logring::sync`], for example before it powers down on purpose;
- a record of an [[Immediate|durability]] track arrives.

The end of a flush writes what the stage buffer holds, even when it is not full. A flush that ends early thus bounds the
loss as well. A cut costs at most the smallest of three: the stage buffer, `flush_bytes` and the interval's data. The
interval matters on a quiet ring, where the stage buffer would take minutes to fill.

The table shows the drive's first minute with four flush sizes. Each row is a run of the drive example on the
simulated card, as the example's `story` prints it. The last column counts the frames of `can0` that waited in RAM at
most at once. A cut at the worst moment would have cost that many.

| `flush_bytes` | Flushes | By the tick | Data on the card | Of it padding | Most frames in RAM |
|---|---|---|---|---|---|
| 2 KiB | 516 | 0 | 1261 KiB | 225 KiB | 118 |
| 8 KiB | 138 | 0 | 1065 KiB | 44 KiB | 238 |
| 16 KiB | 70 | 0 | 1032 KiB | 13 KiB | 238 |
| 32 KiB | 69 | 47 | 1039 KiB | 21 KiB | 238 |

From 8 KiB on, the stage buffer bounds the loss, and the flush size no longer changes it. What the flush size changes
is the room on the card. A smaller flush writes more padding and more flush heads, so the same minute takes more of
the card. At 32 KiB the interval ends most flushes before they fill, and the minute takes a little more room than at
16 KiB. The drive therefore takes 16 KiB, the size with the least room taken.

[`Logring::set_flush`] changes both values while the firmware runs. A drive could flush more often while a test runs,
and less often while it idles.

## Immediate tracks

Some records are worth a flush of their own. The drive's mode changes at each start and stop of the motor. A reader
of a fault wants to know the mode right before it. The `mode` track is therefore Immediate. Its record ends
the flush, so it reaches the card before [`Drive::mode`] returns, together with whatever else waited in the stage
buffer.

An Immediate record costs a write to the card and the padding of the flush it ends, up to a block. That suits a track
with a few records a minute, not the `can0` track with a thousand a second. The [[system track]] is always
Immediate, so a session start, a recording and a marker are on the card when their call returns.

## What a power cut costs

The drive example cuts the power during the first minute. Its simulated card stops after it has written 655 560
bytes, in the middle of a block, 37 seconds into the run. The block it was writing holds a damaged byte, as a block
does when the power fails while the card programs it.

<figure class="ww-figure" markdown="0">
--8<-- "docs/plots/power_cut.svg"
<figcaption>The segment the power cut tore, and the segment the next mount opened. The flush being written lost its
torn block and what came after it. Everything before it stayed. Drawn by <code>logring_figures</code> from the drive
example's power cut. Its segments are the simulated card's 64 KiB, not a real card's 4 MiB.</figcaption>
</figure>

The firmware had pushed 37 046 frames when the power went. After the next mount the card holds 36 958 of them, so the
cut cost 88 frames. Both changes of mode are on the card, since the `mode` track is Immediate. The next mount counts
1 torn block and 1 torn record and writes both counts into the new session's first record.

The cut cost fewer frames than the 238 that the stage buffer can hold. It came while the stage buffer was on its way to
the card. The blocks of it that reached the card whole count. The call [`Logring::durable`] tells the
firmware how far a track's records are safe on the card at any moment.

## How the next mount finds the end

The next boot's [[mount]] does not search the card. It reads the [[superblock]] for a hint of the newest segment and
checks a few segments past it. Inside the newest segment it finds the last good block with a binary search. That
works because the written blocks form a prefix of the segment, and each block's tag and CRC say whether it belongs.

From the last good block, the trailer's distance leads back to the start of its flush. That flush's head gives every
track's position, and the mount reads the flush's records to the end. Where a record breaks off, the mount counts a
torn record. A block whose tag fits but whose CRC fails is a [[torn block]].

The mount then starts the new session in the next segment, whose seq 10 follows the torn one's. It never writes into
the torn segment again until the ring comes round to it. The [format reference](format.md#8-mount-and-recovery) lists
every step of the mount and the bounds on what it reads.
