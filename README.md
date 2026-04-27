# 🛡️ guarden

[![Crates.io](https://img.shields.io/crates/v/guarden.svg)](https://crates.io/crates/guarden)
[![Documentation](https://docs.rs/guarden/badge.svg)](https://docs.rs/guarden)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

`guarden` is a powerful, zero-cost abstraction for scoped guards, deferred execution, and background task detachment in Rust. 

Whether you need to run cleanup logic at the end of a scope, defer asynchronous operations, or seamlessly detach unfinished tasks to a background executor (anti-cancellation), `guarden` provides an ergonomic, macro-driven API that gets out of your way.

## ✨ Highlights

- **🔒 Scoped Cleanup (`guarded!`, `defer!`)**: Bind a guard to a local scope that automatically triggers its execution upon being dropped.
- **⚡ Manual Triggers (`guard!`)**: Create standalone guard values that can be passed around and explicitly triggered.
- **🔄 Universal Execution**: First-class support for both synchronous and `async` bodies. 
- **🚀 Zero-Cost Anti-Cancellation**: In `async` mode, tasks are polled inline by default to avoid scheduling overhead. If the scope is interrupted or cancelled, the remaining work is seamlessly detached to a Tokio background spawner.
- **📦 Advanced Captures**: Granular control over variable captures, including mutable aliases, explicit initialization, and strict export visibility (`export(wrapped)`, `export(all)`).

## 📦 Installation

Add `guarden` to your `Cargo.toml`:

```toml
[dependencies]
guarden = "0.1"
```

## 🚀 Quick Start

### Basic Deferred Cleanup
Ensure cleanup code is executed regardless of how the scope exits (e.g., normal return, early return, or panic).

```rust
use guarden::defer;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

let sink = Arc::new(AtomicUsize::new(0));

{
    defer!([sink = sink.clone()] {
        sink.store(1, Ordering::SeqCst);
    });
    
    // ... do some work ...
    // The guard is automatically triggered when it goes out of scope here.
}

assert_eq!(sink.load(Ordering::SeqCst), 1);
```

### Manual Trigger with Mutable Captures
Capture variables mutably and execute the guard exactly when you want.

```rust
use guarden::guard;

let mut guard = guard!(export(wrapped) [
    mut text = String::from("Hello")
] {
    text.push_str(" World!");
    println!("{}", text);
});

// Mutate captured variables before triggering
guard.text.push_str(",");

// Explicitly trigger the guard
guard.trigger(); 
// Prints: "Hello, World!"
```

### Async Task Detachment (Anti-Cancellation)
Protect critical asynchronous operations from being cancelled. If the parent scope drops the task early, `guarden` automatically detaches it to the background!

```rust
use guarden::guarded;
use tokio::time::{sleep, Duration};

#[tokio::main]
async fn main() {
    let _task = guarded!([] async move {
        // This critical work will complete in the background 
        // even if the parent scope is dropped early!
        sleep(Duration::from_millis(50)).await;
        println!("Background work finished!");
    });
    
    // If we drop _task here, it immediately spawns onto the Tokio runtime.
}
```

## 📚 Documentation

For complete macro syntax, advanced options, and deep-dives into how `guarden` manages type-inference and background task detaching, please check out the [full documentation on docs.rs](https://docs.rs/guarden).

## 📄 License

This project is licensed under the [MIT License](LICENSE).
