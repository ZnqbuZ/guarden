# guarden

`guarden` is the main library crate in the workspace. It re-exports the
public macros for scoped cleanup and deferred execution:

- `guarded!` - bind a guard to a local variable and run its body when dropped.
- `guard!` - create a guard value that you can trigger manually.
- `defer!` - an alias for `guarded!`.

The crate supports both synchronous and asynchronous bodies, explicit capture
lists, and export control for captured values.

## Example

```rust
use guarden::guarded;

let sink = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));

{
    guarded!([sink = sink.clone()] {
        sink.store(7, std::sync::atomic::Ordering::SeqCst);
    });
}

assert_eq!(sink.load(std::sync::atomic::Ordering::SeqCst), 7);
```

## Notes

For more background and workspace-level usage, see the root [`README.md`](../README.md).

## License

MIT

