# guarden-macros

[![Crates.io](https://img.shields.io/crates/v/guarden-macros.svg)](https://crates.io/crates/guarden-macros)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

Procedural macro backend for [guarden](https://crates.io/crates/guarden), a Rust
library for scoped cleanup and async task detachment.

## Using guarden

Add the public library to your dependencies:

```toml
[dependencies]
guarden = "0.3"
```

`guarden` re-exports `guard!`, `guarded!`, and `defer!`, and provides the guard and
task types used by their expansions. Applications do not need a direct dependency
on `guarden-macros`.

See the [quick start](https://github.com/ZnqbuZ/guarden#installation) and
[API documentation](https://docs.rs/guarden) for examples.

## Implementation

This crate exposes the internal `__guarded!` macro. It parses capture lists,
binding options, and export modes, then generates context storage and closures
that use the runtime types in `guarden`.

Both crates live in the [same workspace](https://github.com/ZnqbuZ/guarden) and
share its version. The internal macro interface is intended for use by `guarden`.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](https://github.com/ZnqbuZ/guarden/blob/HEAD/LICENSE-APACHE))
- MIT license ([LICENSE-MIT](https://github.com/ZnqbuZ/guarden/blob/HEAD/LICENSE-MIT))

at your option.
