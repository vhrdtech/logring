---
level: advanced
---

# When the ring is full

The [[drive]]'s card holds the last 20.8 days of the bench. On the next day, the [[data ring]] has no
free [[segment]] left, and the next flush needs one. This chapter answers what the writer does then. By default it
overwrites the oldest segment and goes on; it can instead refuse new records and keep the oldest. The chapter also
shows how the writes spread over the card as the ring goes round.

## Going round

The writer numbers every data segment it opens with a [[segment seq]], from 0 for the life of the log. In a data ring
of `n` slots, the segment with seq `s` goes to the slot `s mod n` of the ring. Seq `n` is the first that finds no free
slot, so it lands over seq 0 in the first slot. Each segment the writer opens after that replaces the oldest one on the card.

<figure class="ww-figure" markdown="0">
--8<-- "docs/plots/ring_laps.svg"
<figcaption>Segment seqs of a ring that went round. Every seq lands in slot seq mod n, over the seq one
lap before it, so the card holds the last n seqs. Drawn by <code>logring_figures</code> from the drive's first minute,
with the ring shrunk to 6 data segments of 64 KiB.</figcaption>
</figure>

The drive example runs ten minutes of the bench on its simulated card. Its ring has 59 data segments. The card holds
seqs 102 to 160 at the end of the run. On the way, the writer overwrote 102 segments, as the superblock's
`segments_reused` counts.

Nothing else changes when the ring goes round. The writer opens and fills segments as before. Every segment still
starts with its [[track table]], so every segment still decodes on its own. A reader on a personal computer (PC) finds
the oldest seq on the card and reads from there. The [[positions|position]] of its first records are no longer 0, which a reader never assumes.

## Refusing new data

Some logs must keep their beginning. An acceptance test, say, wants the first fault and every frame before it, not the
last 20.8 days. For such a log the firmware sets `full` in the [`Config`] to `FullPolicy::DropNewest`. The writer then
fills the data ring once and refuses every record after that: a push returns [`logring::Error::Full`].

The drive example runs the same ten minutes with that policy. The ring took records until 219.9 seconds into the run,
and the card keeps the first 219 943 frames. The drive's bench stops on the error; a firmware that keeps running
instead counts what it refused.

The firmware passes the policy at every mount, and the card does not store it. A firmware that switches to drop newest
on a card that went round gets a full ring at once.

## How wear spreads

Flash wears out with each erase and write. A ring spreads its writes as evenly as a store can. The writer writes each
block of the data ring once per lap and never rewrites one in place. On the drive's card one lap takes the 20.8 days
that [the card's chapter](sd_card.md) found. Each data block thus sees one write per lap.

Two small structures are the exception and take a write at the same place again and again. The writer puts the
[[superblock]] into its two slots in turn. It does so at each mount, at each recording start and stop, and every few
data segments. In the ten minutes above it wrote the superblock 17 times, while the data ring went up to seq 160 in the same
time. The [[meta area]] takes a record at each mount and at each recording start and stop.

A Secure Digital (SD) card spreads its own wear inside. Its controller maps each write to fresh flash, so the
superblock's slots do not wear the same cells over and over. What the host controls is the pattern of its writes.

The ring writes each [[allocation unit]] whole and in
order, the pattern a card handles best. On a raw NOR (logical not-or) flash chip, which has no such controller, the writer erases each
segment before it reuses it. [The format reference](format.md#segment-seqs-on-nor-flash) says how a mount handles a
power cut during that erase.
