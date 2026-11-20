---
level: basic
---

# For writers

This page is for whoever writes or changes a chapter of this book. It says how a chapter reads, how it marks terms
and code, and how its code and figures stay true. A reader of the book can skip it.

## How a chapter reads

A chapter answers one question, and its title names it: "Flushes and power cuts", not "Durability". It opens with the
running example's situation and the question, in two or three sentences. The reader then knows why the chapter
matters before it explains anything.

Why and what come before how. Each new idea gets a concrete case from the running example. Where the idea has a
mechanism, a figure follows right after the sentence that needs it.

A paragraph carries one idea and its reason, in two to five sentences. A list holds parallel items only, and a table
holds a comparison only. Reasoning stays in prose.

Reference detail goes to the [format reference](format.md) and the chapter links it by its numbered section. Field
offsets, bounds, constants and the edge cases of a recovery are such detail. A chapter names a constant only when the
reader picks its value.

A chapter ends on its last fact, or on one sentence that leads to the next chapter. It has no list of further pages.

## The running example

One device runs through the whole book: a motor controller board, the drive. In the field it keeps the last days of
four streams on its Secure Digital (SD) card, each a track of the ring.

| Track | What | Rate |
|---|---|---|
| `log` | the firmware's log lines | a few per second |
| `mode` | the controller's mode: idle, run or fault, on the card at once | on every change |
| `can0` | frames on the vehicle bus | about 1000 per second |
| `vbat` | the battery voltage in mV | once a second |

The example is code, not prose. The crate `logring_example` holds the drive's API, its logger and a bench run of any
length, on a card simulated in RAM. It builds for a personal computer (PC) and for the microcontroller (MCU) of the
drive, `thumbv8m.main-none-eabihf`. The code a chapter shows is thus code a firmware runs.

A number in the story comes from running code, never from a hand calculation. The records in a flush and the bytes a
power cut costs are such numbers. The example's `story` module runs the guide's scenes and prints every number they
give (`cargo run -p logring_example --features std --example story`). A test in `logring_figures` finds each number on
its page, so a change to the example fails until the page says the new number.

## Reading levels

Each page carries a reading level in its front matter, and a badge above its title shows it. [Reading
levels](levels.md) says what the three levels mean.

```yaml
---
level: basic
---
```

The value is `basic`, `advanced` or `nomicon`. Each part of the navigation lists its basic chapters first.

## Terms and code links

A term the book defines has one entry in `docs/glossary.toml`: its name, its other forms, one sentence and the
chapter that explains it. The page [Glossary](glossary.md) lists every entry. In prose, `[[flush]]` marks the term,
and `[[what one write puts down|flush]]` shows other words for it. Mark a term at its first use in each section, not
at every use.

A name in square brackets and backticks links code. ``[`Logring::flush_if_due`]`` links an item, ``[`drive.rs`]`` a
file, and ``[`drive.rs:mount`]`` a marked section of a file. The reader sees the code in a card and opens its lines
on GitHub.

`just book-refs` checks every mark and link, and every build runs it. An unknown term, or a name that no item matches,
fails the build. A renamed item thus fails the book in the same change that renames it.

The book uses one name for each thing. Where two words could name one thing, the glossary entry decides, and the other
word goes into its forms.

## Code on a page

Code on a page always comes from code that the tests compile and run. Two forms carry it.

A short example is a `rust` block, which runs as a doctest. It shows every line it runs and checks its result with
an assertion. For example, this block runs with `cargo test --doc`:

```rust
// A block holds 504 bytes of payload and an 8-byte trailer.
assert_eq!(logring::format::BLOCK, 512);
assert_eq!(logring::format::PAYLOAD, 504);
```

A doctest runs only on a page listed in `crates/logring_example/src/book.rs`. List the page there when you add its
first `rust` block.

Longer code lives in the drive example or its tests, and the page includes a marked section of it. The code marks the
section with `// --8<-- [start:NAME]` and `// --8<-- [end:NAME]` lines. The page shows it in an `rs` block, which
highlights as Rust but does not run as a doctest:

````markdown
```rs
--8<-- "crates/logring_example/src/drive.rs:mount"
```
````

The build fails when the file or the marked section is gone.

## Figures

A figure stands right after the sentence that needs it. Its caption states what the reader should take from it.

A figure that shows bytes or numbers is generated. `logring_figures` draws it from a ring that the drive example
writes, and `just book-figures` writes it into `docs/plots`. A test fails when a drawn figure differs from the file in
git. The caption names the generator, and also what it shrank, such as segments of 8 KiB instead of 4 MiB.

A drawing by hand is an SVG file in `docs/figures`. Both kinds go into the page through a `--8<--` line inside a
`figure` element, so they follow the light and dark scheme.

## Checks

`just book-check` builds the book with warnings as errors and then runs the prose lints of `spirit_level.toml`. The
lints hold a paragraph to five sentences and a sentence to 25 words. A stretch of prose ends within 400 words at a
heading, figure, table or code block.

The format reference is exempt from the reading time of a guide page, since a reader looks things up there.
