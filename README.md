# guarden

[![Crates.io](https://img.shields.io/crates/v/guarden.svg)](https://crates.io/crates/guarden)
[![Documentation](https://docs.rs/guarden/badge.svg)](https://docs.rs/guarden)
[![CI](https://github.com/ZnqbuZ/guarden/actions/workflows/rust.yml/badge.svg)](https://github.com/ZnqbuZ/guarden/actions/workflows/rust.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](https://github.com/ZnqbuZ/guarden/blob/master/LICENSE)

**Defer cleanup. Await inline. Detach on cancellation.**

`guarden` combines scoped cleanup with async work that can outlive the caller
waiting for it. Its guards keep captured state accessible until you choose to
execute the action, let scope exit trigger it, or cancel it and take your values back.

## What makes it useful?

- **Await inline; spawn on cancellation.** Async work runs as part of the caller's
  task. If the caller drops the wait, the unfinished future is transferred to the
  spawner. Work that finishes inline needs no initial background spawn.
- **Edit captured state before cleanup.** Access captures through local references
  or named guard fields. `.defuse()` cancels the action and gives the context back.
- **The same guard lifecycle for sync and async.** Drop it for automatic cleanup,
  or `.trigger()` it explicitly; async triggers return an awaitable task.
- **Small, explicit costs.** Sync guards need no heap allocation of their own.
  Async tasks use one pinned allocation; boxed async guards reuse one allocation
  for the action and its future.
- **Runtime-independent core.** Sync guards and custom-spawner tasks work with
  `no_std` + `alloc`. Tokio supplies the default adapter; other executors plug in
  through `TaskSpawner`. Without Tokio, the default `()` spawner returns tasks
  unchanged; inline execution remains available, but no background work is scheduled.

For example, change an async guard's state before running it:

```rust
use guarden::guard;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let mut work = guard!(export(wrapped) [mut message = String::from("Hello")] async move {
        tokio::task::yield_now().await;
        println!("{message}");
    });

    work.message.push_str(", world!");
    work.trigger().await; // Poll inline; detach if this wait is cancelled.
}
```

Detached work needs a running executor. See the
[timeout example](#continue-async-work-after-a-timeout) for a complete demonstration.

[API documentation](https://docs.rs/guarden) ·
[Source](https://github.com/ZnqbuZ/guarden) ·
[Issues](https://github.com/ZnqbuZ/guarden/issues)

## Installation

Requires **Rust 1.95 or newer**. The examples below target the repository's `0.4` API.

```toml
[dependencies]
guarden = "0.4"
```

Tokio integration is enabled by default. To run the async example below, also add
Tokio with the runtime, macro, timer, and channel features used in this README:

```toml
tokio = { version = "1", features = ["rt", "macros", "time", "sync"] }
```

## Choose a macro

| Macro | What it creates | Typical use |
| --- | --- | --- |
| `defer!` | A guard bound to the current scope; alias for `guarded!` | Run cleanup at scope exit |
| `guarded!` | A scope-bound guard, optionally named with `name =>` | Access captures or trigger cleanup early |
| `guard!` | A guard value returned as an expression | Store, pass, trigger, or defuse a guard |

All three run their action when the guard is dropped, unless it has already been
consumed by `.trigger()` or `.defuse()`. For async actions, dropping the guard
creates the future and hands it to the spawner; it does not wait for completion.

`guarded!` and `defer!` are statements. To obtain a value, write
`let cleanup = guard!(...);`. Keep the binding: `let _ = guard!(...);` drops the
guard immediately.

### Cleanup at scope exit

```rust
use guarden::defer;

let mut events = Vec::new();
{
    defer!([events = &mut events] {
        events.push("scope exited");
    });
    // Work here. Cleanup also runs on early return or a `?` error.
}
assert_eq!(events, ["scope exited"]);
```

## Control cleanup and captured values

Use `export(wrapped)` to access captures as fields without introducing new local
bindings:

```rust
use guarden::guard;

let mut cleanup = guard!(export(wrapped) [
    mut message = String::from("Hello")
] {
    message.push_str(" world!");
    assert_eq!(message, "Hello, world!");
    println!("{message}");
});

cleanup.message.push(',');
cleanup.trigger(); // Runs now and consumes the guard.
```

When cleanup is no longer needed, `.defuse()` skips the action and returns the
captured context. With one capture and no `export(wrapped)`, this is the value itself:

```rust
use guarden::guard;

let cleanup = guard!([buffer = String::from("ready to keep")] {
    println!("Discarding: {buffer}");
});

let buffer = cleanup.defuse(); // The action above does not run.
assert_eq!(buffer, "ready to keep");
```

## Continue async work after a timeout

An ordinary future is cancelled when it is dropped. With `guarden`,
`guard.trigger().await` polls the work inline, and dropping that wait before
completion hands the remaining work to Tokio.

This example holds an operation open with a channel so that the timeout happens
before it can finish. It then lets the detached operation complete and observes
its result:

```rust
use guarden::guard;
use tokio::sync::oneshot;
use tokio::time::{timeout, Duration};

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let (release_tx, release_rx) = oneshot::channel::<()>();
    let (done_tx, done_rx) = oneshot::channel();

    let operation = guard!([release_rx, done_tx] async move {
        // Stand in for an async operation that is not ready yet.
        release_rx.await.expect("operation released");
        let _ = done_tx.send("finished");
    });

    let result = timeout(Duration::from_millis(10), operation.trigger()).await;
    assert!(result.is_err()); // The caller stopped waiting; work was detached.

    release_tx.send(()).expect("operation still alive");
    assert_eq!(done_rx.await.unwrap(), "finished");
}
```

The example waits for the completion signal to keep the runtime alive. Detachment
protects against the caller dropping the future; it does **not** guarantee
completion after runtime shutdown, process exit, or a panic in the operation.
[Tokio drops outstanding tasks when its runtime shuts down.](https://docs.rs/tokio/latest/tokio/task/fn.spawn.html)

### Async lifecycle

| Action | Behavior |
| --- | --- |
| Create an async guard | Store its context and action; no task is spawned |
| Call `.trigger()` | Create a `DetachableTask`; no task is spawned yet |
| Await that task | Poll the future inline and return its output |
| Drop an untriggered guard | Create the future and hand it to the spawner |
| Drop a triggered task, or its pending wait | Hand the unfinished future to the spawner |
| Call `.defuse()` on the guard | Recover the context without creating the future |

With the default Tokio spawner, the future and its output must be `Send + 'static`.
Detachment must happen inside a Tokio runtime context. Pass owned data into async
work, and handle errors inside the operation if its result may be discarded.
Task-local or thread-local state is not automatically carried into the detached task.

## Runtime support

| API | Without `tokio` | With `tokio` (default) |
| --- | --- | --- |
| Synchronous guards and macros | Available | Available |
| `DetachableTask::new(spawner, future)` / `from_boxed(spawner, future)` | Explicit spawner | Explicit spawner |
| `ContextGuard::with_spawner(context, spawner, action)` | Explicit spawner | Explicit spawner |
| Inferred async macro bodies / `DetachableTask::from(future)` | Await inline; discard unfinished work on detachment | Await inline; spawn unfinished work on detachment |
| `DefaultSpawner` / `DEFAULT_SPAWNER` | Use `()` (identity) | Use `TokioHandle` |
| Explicit `()` spawner | Available | Available |

For the `no_std` + `alloc` core:

```toml
guarden = { version = "0.3", default-features = false }
```

Async guards and tasks still execute inline while awaited. Without Tokio, the
default `()` spawner returns the pinned task unchanged, without polling or
scheduling it. A direct `().spawn(task)` call lets the caller retain that task.
Guard detachment ignores the return value, so in that path the returned task is
dropped and its unfinished work is cancelled. No fallback-specific warning is emitted.

To continue detached work on another executor, implement
[`TaskSpawner`](https://docs.rs/guarden/latest/guarden/task/trait.TaskSpawner.html)
or pass a closure that transfers the pinned future to your executor or task queue.
The explicit-spawner constructors do not impose Tokio's `Send + 'static` bounds;
your spawner determines those requirements. The `From` convenience implementation
retains its `Send + 'static` bounds in both configurations.

The `()` identity spawner is available in both configurations. A spawner that
continues execution after guard detachment must retain the task independently of
its return value. To take a future back without invoking the spawner, call
`.reclaim()`.

## Capture syntax

Captures support `[value, mut state, alias = expression, mut buffer = expression]`.
Initializers run when the guard is created; the body runs when it is triggered or dropped.

For `guarded!` and `defer!`, export modes control access from the surrounding scope:

| Mode | Access after the macro |
| --- | --- |
| Default | Shorthand captures such as `[value, mut state]` shadow the original names with `&T` or `&mut T`; initialized captures stay private |
| `export(all)` | All captures, including initialized ones, are exposed as references under their capture names |
| `export(wrapped)` | Access captures as fields on a named guard; no local names are exported |

Captures are moved into the guard. To borrow an existing value, use an explicit
reference such as `[state = &mut state]`. `export(wrapped)` changes how you access
the context; it still transfers ownership of captures into the guard.

Options, when present, must appear in this order:

```text
guarded!([mut] name => sync move export(...) [captures] body);
guard!(sync move export(wrapped) [captures] body)
```

Every option before `body` is optional. The brackets around `mut` above denote
optional syntax; `[captures]` is the literal capture list. `guard!` returns a value
and only accepts `export(wrapped)` as an explicit export mode.

Use `sync` to return a non-`()` value synchronously, or to resolve type inference
when the body only contains a diverging expression such as `panic!` or `loop {}`.
See the [macro reference](https://docs.rs/guarden/latest/guarden/macro.guarded.html)
for named bindings and more examples.

## Costs and execution boundaries

Synchronous guards store their context and action directly, without an allocation
by the guard itself. Async tasks use a pinned heap allocation so they can outlive
the caller's stack frame. Inline polling avoids an initial background spawn; it
does not make the async path allocation-free. Opting into `.boxed()` adds type
erasure; boxed async guards share one allocation between the action and its future.

Cleanup follows Rust's `Drop` semantics: it runs on normal scope exit, early
return, and panic **unwinding**. It does not run when destructors are skipped, such
as with `panic = "abort"` or process exit. This workspace's release profile uses
`panic = "abort"`; downstream applications choose their own panic strategy.

## More APIs

- [ContextGuard](https://docs.rs/guarden/latest/guarden/guard/struct.ContextGuard.html):
  construct guards directly, recover their parts, or provide a custom spawner with
  `ContextGuard::with_spawner`.
- [DetachableTask](https://docs.rs/guarden/latest/guarden/task/struct.DetachableTask.html):
  wrap an existing future, detach it explicitly, or reclaim the pinned future.
- [Boxed guards](https://docs.rs/guarden/latest/guarden/guard/boxed/index.html):
  erase action types to store guards in structs or collections.
- [TaskSpawner](https://docs.rs/guarden/latest/guarden/task/trait.TaskSpawner.html):
  integrate a custom executor.

## Feedback and contributions

Bug reports, API feedback, and examples of real-world usage are welcome in
[GitHub issues](https://github.com/ZnqbuZ/guarden/issues). For bugs, include a minimal
reproduction, your Rust version, and the enabled features.

To run the workspace tests, including documentation examples:

```sh
cargo test --workspace --locked
cargo test -p guarden --no-default-features --locked
```

## License

[MIT](https://github.com/ZnqbuZ/guarden/blob/master/LICENSE)
