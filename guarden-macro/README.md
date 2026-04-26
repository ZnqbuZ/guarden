# guarden-macro

Procedural macro backend for the `guarden` crate.

This crate provides the low-level `__guarded` proc macro used by the public
macros exported from `guarden`:

- `guarden::guarded!`
- `guarden::guard!`
- `guarden::defer!`

Most users should depend on `guarden` directly. This crate exists so the
public API can stay small and ergonomic while the parsing and expansion logic
remains isolated in a dedicated proc-macro crate.

## What it does

- Parses the `guarded!` / `guard!` macro syntax.
- Expands synchronous and asynchronous guard bodies.
- Supports capture lists, `move`, `sync`, and export modes.
- Generates the runtime glue used by the `guarden` library crate.

## When to use it

Only depend on `guarden-macro` directly if you are working on the macro
implementation itself or building on top of its internal expansion behavior.

## Versioning

`guarden-macro` is intended to track the same version as `guarden`.

## License

MIT

