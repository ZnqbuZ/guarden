# guarden-macros

[![Crates.io](https://img.shields.io/crates/v/guarden-macros.svg)](https://crates.io/crates/guarden-macros)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](https://github.com/ZnqbuZ/guarden/blob/master/LICENSE)

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

[MIT](https://github.com/ZnqbuZ/guarden/blob/master/LICENSE)
