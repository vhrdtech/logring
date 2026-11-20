# logring_figures

This crate draws the figures of the book pages in `docs/` that show bytes, blocks or numbers. The drive example
(`logring_example`) writes a ring on a simulated card, and logring reads it back. Each figure comes from such a ring,
so a figure cannot drift from the format.

The figures are SVG files in `docs/plots/`, kept in git. Their classes (`ww-svg`, `thin`, `faint`, `tint-blue`, ...)
take the book's colours in light and dark from `docs/stylesheets/figures.css`.

```sh
just book-figures   # rewrite docs/plots/*.svg
```

A test (`tests/it/current.rs`) draws every figure again and fails when a file in `docs/plots/` differs. A format
change that moves a byte therefore also asks for the figures to be redrawn. Drawings that hold no numbers are drawn by
hand in `docs/figures/`.
