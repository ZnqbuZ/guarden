# ⚙️ guarden-macro

[![Crates.io](https://img.shields.io/crates/v/guarden-macro.svg)](https://crates.io/crates/guarden-macro)
[![Documentation](https://docs.rs/guarden-macro/badge.svg)](https://docs.rs/guarden-macro)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](../LICENSE)

The procedural macro backend for the [`guarden`](https://crates.io/crates/guarden) crate.

This crate provides the low-level `__guarded!` procedural macro engine that powers the public, ergonomic macros exported by `guarden`:
- `guarded!`
- `guard!`
- `defer!`

> **⚠️ Note for Users:**  
> You should **not** depend on this crate directly. Please depend on the main [`guarden`](https://crates.io/crates/guarden) crate, which re-exports everything you need and provides the necessary runtime context and traits.

## 🧠 How it Works

The `guarden-macro` engine is responsible for parsing custom syntax and expanding it into highly optimized, zero-cost Rust abstractions. Its capabilities include:

- **Syntax Parsing**: Robustly parsing capturing clauses (e.g., `[mut value, state = state.clone()]`) and complex export modes (`export(wrapped)`, `export(all)`).
- **Type-Inference Gymnastics**: Emitting smart compiler hints (like generating generic closures bounded by `FnOnce(Context) -> _R`) to flawlessly distinguish between synchronous blocks and `async` futures without requiring explicit type annotations from the user.
- **Scope Rewriting**: Generating hidden, local structs (ZSTs and capturing structs) to hold state locally, ensuring memory locality and deferring background spawns strictly as a fallback.

## 📦 Versioning

This crate is tightly coupled to `guarden`. It tracks the exact same version number as the parent crate and is released in lockstep.

## 📄 License

This project is licensed under the [MIT License](../LICENSE).
