# guarden

`guarden` is a small Rust workspace for scoped guards and deferred cleanup.
It provides ergonomic macros for running synchronous or asynchronous cleanup
logic when a scope ends or when you trigger the guard manually.

## Workspace crates

- `guarden` - the main library crate that re-exports the public macros.
- `guarden-macro` - the procedural macro backend used internally by `guarden`.

## Highlights

- `guarded!` binds a guard to a local variable and runs the body when dropped.
- `guard!` returns a guard value you can trigger manually.
- `defer!` is a convenient alias for `guarded!`.
- Supports sync and async bodies, explicit capture lists, and export controls.

## Quick start

Add the main crate to your project:

```toml
[dependencies]
guarden = { version = "0.0.1", path = "../guarden" }
```

### Scoped cleanup

```rust
use guarden::guarded;

let sink = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));

{
    guarded!([sink = sink.clone()] {
        sink.store(1, std::sync::atomic::Ordering::SeqCst);
    });
}

assert_eq!(sink.load(std::sync::atomic::Ordering::SeqCst), 1);
```

### Manual trigger

```rust
use guarden::guard;

let guard = guard!([value = 42usize] {
    let _ = value;
});

guard.trigger();
```

## Crate docs

- Main crate docs: [`guarden/README.md`](guarden/README.md)
- Macro backend docs: [`guarden-macro/README.md`](guarden-macro/README.md)

## License

MIT

