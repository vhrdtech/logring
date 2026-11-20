//! The book's pages (`docs/`) as doctests.
//!
//! Every `rust` block of a page listed here runs with `cargo test --doc`. Code that a page includes from a source
//! file is fenced `rs` and is not a doctest. A page with a `rust` block has a line here (docs/writers.md, "Code on a
//! page").

macro_rules! pages {
    ($($name:ident = $path:literal;)*) => {$(
        #[doc = include_str!(concat!("../../../docs/", $path))]
        pub struct $name;
    )*};
}

pages! {
    Writers = "writers.md";
}
