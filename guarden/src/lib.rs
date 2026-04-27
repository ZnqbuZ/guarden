//! `guarden` provides scoped guard macros for deferred cleanup and manual triggers.
//!
//! The public API centers on three macros:
//!
//! - [`guarded!`] for binding a guard to a local variable that runs on Drop.
//! - [`guard!`] for creating a guard value that can be triggered manually.
//! - [`defer!`] as a convenience alias for [`guarded!`].
//!
//! The macros support synchronous and asynchronous bodies, explicit capture lists,
//! and export controls for captured values.
//!
//! ### ⚠️ Critical Usage Note: Diverging Expressions
//!
//! Do not use "naked" diverging expressions—such as `panic!`, `todo!`, or `loop {}`—as
//! the sole content of a sync guard closure. This prevents the compiler from
//! distinguishing between synchronous (`ASYNC = false`) and asynchronous
//! (`ASYNC = true`) implementations, leading to a type inference error (E0277).
//!
//! ### Technical Context
//!
//! The `!` (Never Type) is a bottom type that can be coerced into any other type.
//! Because it satisfies both the `()` return type requirement for sync guards and the `Future`
//! trait requirement for async guards, the compiler encounters an inference deadlock.
//!
//! ### Workaround
//!
//! For macros like `guard!` or `guarded!`, force the closure to resolve to `()`
//! by explicitly setting the guard to `sync`:
//!
//! ```rust,should_panic
//! let val = "critical failure".to_string();
//! guarded! {
//!     sync [val] {
//!         panic!("{}", val);
//!     }
//! }
//! ```

extern crate self as guarden;

pub mod guard;
pub mod task;

#[doc(hidden)]
pub use guarden_macro::__guarded;

/// Creates a [`ContextGuard`](guard::ContextGuard) object, binding it to a variable within the local scope.
///
/// ### Examples
///
/// ```rust
/// # use guarden::guarded;
/// let v1 = "1".to_string();
/// let v2 = "2".to_string();
/// let mut v4 = "4".to_string();
/// {
///     let v5 = "5".to_string();
///     guarded! {
///         guard => sync move export(all) [
///             v1,
///             mut v2,
///             v3 = "3".to_string(),
///             mut v4 = &mut v4
///         ] {
///             v2 += &v1;
///             *v4 += &v2;
///             *v4 += &v3;
///             *v4 += &v5;
///             assert_eq!(v2, "2.1");
///         }
///     }
///     *v2 += ".";
///     **v4 += ".";
///     assert_eq!(v1, "1");
///     assert_eq!(v2, "2.");
///     assert_eq!(v3, "3");
///     assert_eq!(*v4, "4.");
/// }
/// assert_eq!(v4, "4.2.135");
/// ```
///
/// #### Options
///
/// > **Syntax Order:** The macro requires options to appear in the exact order shown below if they are used.
///
/// * `[mut] guard =>` (**Optional**): The name of the variable to which the guard will be bound. Can be prefixed with `mut` to allow mutable access to the guard. If omitted, a default hidden variable is used, and the guard will be automatically triggered at the end of the current scope.
/// * `sync` (**Optional**): Forces the guard to be evaluated synchronously. Essential for avoiding type inference deadlocks when using diverging expressions (like `panic!`) as the sole content of the closure.
/// * `move` (**Optional**): Forces the underlying closure to take ownership of the captured variables.
/// * `export(all)` | `export(wrapped)` (**Optional**): Controls which captured variables are re-exported (made accessible) to the surrounding scope after the macro invocation.
///   * `export(all)`: Re-exports all captured variables, including explicitly initialized ones (e.g., `a = a.clone()`). **Note: This may shadow existing local variables in the outer scope.**
///   * `export(wrapped)`: Wraps all captured variables in a local struct, which can be accessed via `Deref`/`DerefMut` of the guard. No bindings are exported to the outer scope.
///   * **Default** (when omitted): Only shorthand captures (`mut arg` or `arg`) are re-exported.
///
///   > **⚠️ WARNING on Implicit Export Shadowing:**
///   > By default, or when using `export(all)`, captured variables are physically *moved* into the guard
///   > and then re-exported back to the outer scope as **references** (e.g., `&mut T` or `&T`) that shadow
///   > the original bindings. This alters their type and ownership semantics. If you need to preserve
///   > original ownership, either capture them explicitly as references (`a = &mut a`), or use `export(wrapped)`.
///
/// * `[ ... captures ... ]` (**Optional**): A comma-separated list of context variables to capture and make available within the guard. Supports:
///   * `mut arg = expr` (Mutable initialization)
///   * `arg = expr` (Immutable initialization)
///   * `mut arg` (Mutable shorthand capture)
///   * `arg` (Immutable shorthand capture)
/// * `{ ... }` or `expr` (**Required**): The body of the guard to be executed when triggered.
///
/// **Note:** For usage with `panic!` or `loop`, see the [module-level documentation](self)
/// regarding type inference deadlocks.
///
/// #### Named binding + mut arg visible outside + explicit drop
/// ```rust
/// # use guarden::guarded;
/// # use std::sync::atomic::{AtomicUsize, Ordering};
/// # use std::sync::Arc;
/// let sink = Arc::new(AtomicUsize::new(0));
/// {
///     let mut value = 6usize;
///     let delta = 1usize;
///     guarded!(guard => [mut value, delta, sink = sink.clone()] {
///         sink.store(value + delta, Ordering::SeqCst);
///     });
///
///     *value += 10;
///     assert_eq!(*value, 16);
///     assert_eq!(*delta, 1);
///     drop(guard);
/// }
/// assert_eq!(sink.load(Ordering::SeqCst), 17);
/// ```
///
/// #### Unnamed statement + expression body + implicit drop at scope end
/// ```rust
/// # use guarden::guarded;
/// # use std::sync::atomic::{AtomicUsize, Ordering};
/// # use std::sync::Arc;
/// let sink = Arc::new(AtomicUsize::new(0));
/// {
///     guarded!([n = 7usize, sink = sink.clone()] sink.store(n, Ordering::SeqCst));
/// }
/// assert_eq!(sink.load(Ordering::SeqCst), 7);
/// ```
///
/// #### Explicit sync + panic path propagates on drop
/// ```rust
/// # use guarden::guarded;
/// let dropped = std::panic::catch_unwind(|| {
///     guarded! {
///         sync {
///             panic!("boom");
///         }
///     }
/// });
/// assert!(dropped.is_err());
/// ```
///
/// #### Async inference + detaches and executes on drop
/// ```rust
/// # #[cfg(feature = "tokio")]
/// # {
/// # tokio_test::block_on(async {
/// # use guarden::guarded;
/// let (tx, rx) = tokio::sync::oneshot::channel();
/// {
///     let tx = Some(tx);
///     guarded!([mut tx] {
///         let tx = tx.take();
///         async move {
///             if let Some(tx) = tx {
///                 let _ = tx.send(9usize);
///             }
///         }
///     });
/// }
/// let detached = tokio::time::timeout(std::time::Duration::from_secs(1), rx)
///     .await
///     .expect("detached task should complete")
///     .expect("detached task should send value");
/// assert_eq!(detached, 9);
/// # })
/// # }
/// ```
///
/// #### Init captures stay private and do not shadow outer locals
/// ```rust
/// # use guarden::guarded;
/// # use std::sync::atomic::{AtomicUsize, Ordering};
/// # use std::sync::Arc;
/// let sink = Arc::new(AtomicUsize::new(0));
/// {
///     let mut total = 100usize;
///     let step = 200usize;
///
///     guarded!(guard => [mut total = 10usize, step = 3usize, sink = sink.clone()] {
///         sink.store(total + step, Ordering::SeqCst);
///     });
///
///     total += 2;
///     assert_eq!(total, 102);
///     assert_eq!(step, 200);
///     drop(guard);
/// }
/// assert_eq!(sink.load(Ordering::SeqCst), 13);
/// ```
///
/// #### Async inference + init captures
/// ```rust
/// # #[cfg(feature = "tokio")]
/// # {
/// # tokio_test::block_on(async {
/// # use guarden::guarded;
/// let (tx, rx) = tokio::sync::oneshot::channel();
/// {
///     guarded!([mut tx = Some(tx), value = 13usize] {
///         let tx = tx.take();
///         async move {
///             if let Some(tx) = tx {
///                 let _ = tx.send(value);
///             }
///         }
///     });
/// }
/// let detached = tokio::time::timeout(std::time::Duration::from_secs(1), rx)
///     .await
///     .expect("detached init-capture task should complete")
///     .expect("detached init-capture task should send value");
/// assert_eq!(detached, 13);
/// # })
/// # }
/// ```
///
/// #### Export all captured variables
/// ```rust
/// # use guarden::guarded;
/// # use std::sync::atomic::{AtomicUsize, Ordering};
/// # use std::sync::Arc;
/// let sink = Arc::new(AtomicUsize::new(0));
/// {
///     guarded! {
///         guard => export(all) [
///             mut a = 5usize,
///             b = 4usize,
///             sink = sink.clone()
///         ] {
///             sink.store(a + b, Ordering::SeqCst);
///         }
///     }
///
///     // Both `a` and `b` are exported because of export(all)
///     *a += 5;
///     assert_eq!(*a, 10);
///     assert_eq!(*b, 4);
/// }
/// assert_eq!(sink.load(Ordering::SeqCst), 14); // 10 + 4
/// ```
///
/// #### Wrapped captures accessed via mutable guard
/// ```rust
/// # use guarden::guarded;
/// # use std::sync::atomic::{AtomicUsize, Ordering};
/// # use std::sync::Arc;
/// let sink = Arc::new(AtomicUsize::new(0));
/// {
///     guarded! {
///         mut guard => export(wrapped) [
///             mut a = 5usize,
///             b = 4usize,
///             sink = sink.clone()
///         ] {
///             sink.store(a + b, Ordering::SeqCst);
///         }
///     }
///
///     // Modify 'a' through the guard's DerefMut
///     guard.a += 5;
///     assert_eq!(guard.a, 10);
///     assert_eq!(guard.b, 4);
/// }
/// assert_eq!(sink.load(Ordering::SeqCst), 14); // 10 + 4
/// ```
#[macro_export]
macro_rules! guarded {
    ($($tt:tt)*) => {
        $crate::__guarded!(
            @stmt;
            $($tt)*
        )
    };
}

/// Creates a [`ContextGuard`](guard::ContextGuard) object, without binding it to a variable.
/// The macro evaluates to an expression returning the guard.
///
/// **Note:** For usage with `panic!` or `loop`, see the [module-level documentation](self)
/// regarding type inference deadlocks.
///
/// ### Examples
///
/// ```rust
/// # use guarden::guard;
/// let v1 = "1".to_string();
/// let v2 = "2".to_string();
/// let mut v4 = "4".to_string();
/// {
///     let v5 = "5".to_string();
///     struct Ctx<'s> {
///         v1: String,
///         v2: String,
///         v3: String,
///         v4: &'s mut String,
///     }
///     let mut guard = guard! {
///         sync move [
///             ctx = Ctx {
///                 v1,
///                 v2,
///                 v3: "3".to_string(),
///                 v4: &mut v4,
///             }
///         ] {
///             let Ctx { v1, mut v2, v3, v4 } = ctx;
///             v2 += &v1;
///             *v4 += &v2;
///             *v4 += &v3;
///             *v4 += &v5;
///             assert_eq!(v2, "2.1");
///         }
///     };
///     let Ctx { v1, v2, v3, v4 } = &mut *guard;
///     *v2 += ".";
///     **v4 += ".";
///     assert_eq!(v1, "1");
///     assert_eq!(v2, "2.");
///     assert_eq!(v3, "3");
///     assert_eq!(*v4, "4.");
/// }
/// assert_eq!(v4, "4.2.135");
/// ```
///
/// ##### Options
///
/// > **Syntax Order:** The macro requires options to appear in the exact order shown below if they are used.
///
/// * `sync` (**Optional**): Forces the guard to be evaluated synchronously. Essential for avoiding type inference deadlocks when using diverging expressions (like `panic!`) as the sole content of the closure.
/// * `move` (**Optional**): Forces the underlying closure to take ownership of the captured variables.
/// * `[ ... captures ... ]` (**Optional**): A comma-separated list of context variables to capture and make available within the guard. Supports:
///   * `mut arg = expr` (Mutable initialization)
///   * `arg = expr` (Immutable initialization)
///   * `mut arg` (Mutable shorthand capture)
///   * `arg` (Immutable shorthand capture)
/// * `{ ... }` or `expr` (**Required**): The body of the guard to be executed when triggered.
///
/// #### Block body + trailing comma inits + trigger()
/// ```rust
/// # use guarden::guard;
/// # use std::sync::atomic::{AtomicUsize, Ordering};
/// # use std::sync::Arc;
/// let sink = Arc::new(AtomicUsize::new(0));
/// let guard = guard!([v = 1usize, sink = sink.clone(),] {
///     sink.store(v, Ordering::SeqCst);
/// });
/// guard.trigger();
/// assert_eq!(sink.load(Ordering::SeqCst), 1);
/// ```
///
/// #### Expression body (no braces)
/// ```rust
/// # use guarden::guard;
/// # use std::sync::atomic::{AtomicUsize, Ordering};
/// # use std::sync::Arc;
/// let sink = Arc::new(AtomicUsize::new(0));
/// let guard = guard!([v = 2usize, sink = sink.clone()] sink.store(v, Ordering::SeqCst));
/// guard.trigger();
/// assert_eq!(sink.load(Ordering::SeqCst), 2);
/// ```
///
/// #### Explicit sync + no inits form
/// ```rust
/// # use guarden::guard;
/// # use std::sync::atomic::{AtomicUsize, Ordering};
/// # use std::sync::Arc;
/// let sink = Arc::new(AtomicUsize::new(0));
/// let guard = guard!(sync [sink = sink.clone()] {
///     loop {
///         sink.store(3, Ordering::SeqCst);
///         break;
///     }
/// });
/// guard.trigger();
/// assert_eq!(sink.load(Ordering::SeqCst), 3);
/// ```
///
/// #### Move capture + defuse() prevents execution
/// ```rust
/// # use guarden::guard;
/// # use std::sync::atomic::{AtomicUsize, Ordering};
/// # use std::sync::Arc;
/// let sink = Arc::new(AtomicUsize::new(0));
/// let guard = guard!([
///     owned = String::from("owned"),
///     sink = sink.clone()
/// ] {
///     if owned == "owned" {
///         sink.store(4, Ordering::SeqCst);
///     }
/// });
/// let (owned, _) = guard.defuse();
/// assert_eq!(owned, "owned");
/// assert_eq!(sink.load(Ordering::SeqCst), 0);
/// ```
///
/// #### Async block inference + trigger() returns task
/// ```rust
/// # #[cfg(feature = "tokio")]
/// # {
/// # tokio_test::block_on(async {
/// # use guarden::guard;
/// # use std::sync::atomic::{AtomicUsize, Ordering};
/// # use std::sync::Arc;
/// let sink = Arc::new(AtomicUsize::new(0));
/// let guard = guard!([n = 5usize, sink = sink.clone()] async move {
///     sink.fetch_add(n, Ordering::SeqCst);
/// });
/// guard.trigger().await;
/// assert_eq!(sink.load(Ordering::SeqCst), 5);
/// # })
/// # }
/// ```
///
/// #### Init capture (immutable) + trigger()
/// ```rust
/// # use guarden::guard;
/// # use std::sync::atomic::{AtomicUsize, Ordering};
/// # use std::sync::Arc;
/// let sink = Arc::new(AtomicUsize::new(0));
/// let guard = guard!([value = 10usize, sink = sink.clone()] {
///     sink.store(value, Ordering::SeqCst);
/// });
/// guard.trigger();
/// assert_eq!(sink.load(Ordering::SeqCst), 10);
/// ```
///
/// #### Init capture (mutable) + trigger()
/// ```rust
/// # use guarden::guard;
/// # use std::sync::atomic::{AtomicUsize, Ordering};
/// # use std::sync::Arc;
/// let sink = Arc::new(AtomicUsize::new(0));
/// let guard = guard!([mut text = String::from("a"), sink = sink.clone()] {
///     text.push_str("b");
///     sink.store(text.len(), Ordering::SeqCst);
/// });
/// guard.trigger();
/// assert_eq!(sink.load(Ordering::SeqCst), 2);
/// ```
///
/// #### Wrapped captures returned as expression
/// ```rust
/// # use guarden::guard;
/// # use std::sync::atomic::{AtomicUsize, Ordering};
/// # use std::sync::Arc;
/// let sink = Arc::new(AtomicUsize::new(0));
/// let mut guard = guard!(export(wrapped) [
///     mut text = String::from("a"),
///     sink = sink.clone()
/// ] {
///     text.push_str("b");
///     sink.store(text.len(), Ordering::SeqCst);
/// });
///
/// guard.text.push_str("x");
/// guard.trigger();
/// assert_eq!(sink.load(Ordering::SeqCst), 3); // "ax" + "b" = "axb"
/// ```
#[macro_export]
macro_rules! guard {
    ($($tt:tt)*) => {
        $crate::__guarded!(
            @expr;
            $($tt)*
        )
    };
}

/// Alias for [`guarded!`].
///
/// **Note:** For usage with `panic!` or `loop`, see the [module-level documentation](self)
/// regarding type inference deadlocks.
#[macro_export]
macro_rules! defer {
    ( $($tt:tt)* ) => {
        $crate::guarded! { $($tt)* }
    };
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn init_capture_sync_evaluates_initializer_once() {
        let init_calls = Arc::new(AtomicUsize::new(0));
        let observed = Arc::new(AtomicUsize::new(0));

        let guard = crate::guard!([
            observed = observed.clone(),
            value = {
                init_calls.fetch_add(1, Ordering::SeqCst);
                7usize
            }
        ] {
            observed.store(value, Ordering::SeqCst);
        });

        guard.trigger();

        assert_eq!(init_calls.load(Ordering::SeqCst), 1);
        assert_eq!(observed.load(Ordering::SeqCst), 7);
    }

    #[test]
    fn defer_alias_behaves_like_guarded_statement() {
        let called = Arc::new(AtomicUsize::new(0));

        {
            crate::defer!([n = 42usize, called = called.clone()] {
                called.store(n, Ordering::SeqCst);
            });
        }

        assert_eq!(called.load(Ordering::SeqCst), 42);
    }
}

#[cfg(all(test, feature = "tokio"))]
mod tokio_tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    #[tokio::test]
    async fn defuse_async_does_not_execute() {
        let called = Arc::new(AtomicUsize::new(0));

        let guard = crate::guard!([
            called = called.clone(),
            value = 11usize
        ] async move {
            called.fetch_add(value, Ordering::SeqCst);
        });

        let (_, value) = guard.defuse();
        assert_eq!(value, 11);

        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(called.load(Ordering::SeqCst), 0);
    }
}
