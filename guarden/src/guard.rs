pub mod action;
pub mod boxed;

use crate::task::TaskSpawner;
use action::mode::Mode;
use action::{Action, Infer, Spawn};
use core::fmt;
use core::fmt::Debug;
use core::mem::ManuallyDrop;
use core::ops::{Deref, DerefMut};

struct ContextGuardInner<Context, Action> {
    context: Context,
    action: Action,
}

/// RAII guard that owns a context value and executes a closure on drop.
///
/// You can either:
/// - let `Drop` trigger the guard body automatically,
/// - call [`trigger`](Self::trigger) for eager execution, or
/// - call [`defuse`](Self::defuse) to recover the context without execution.
#[must_use = "if you don't bind a guard to a variable, its action executes immediately (e.g., `let _g = guard!(...);`)"]
pub struct ContextGuard<Context, A: Action<Context>> {
    inner: ManuallyDrop<ContextGuardInner<Context, A>>,
}

impl<Context, A: Action<Context>> Deref for ContextGuard<Context, A> {
    type Target = Context;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.inner.context
    }
}

impl<Context, A: Action<Context>> DerefMut for ContextGuard<Context, A> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner.context
    }
}

impl<Context: Debug, A: Action<Context>> Debug for ContextGuard<Context, A> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ContextGuard")
            .field("context", &self.inner.context)
            .finish_non_exhaustive()
    }
}

impl<Context, A: Action<Context>> ContextGuard<Context, A> {
    /// Creates a new `ContextGuard`.
    ///
    /// The compiler will automatically infer whether the guard is synchronous or
    /// asynchronous based on the closure's return type. If the closure returns
    /// `()`, it will be a synchronous guard. If it returns a `Future`, it will
    /// be an asynchronous guard.
    ///
    /// **Note on generics:** The seemingly unused `_Output` generic parameter and the
    /// `F: FnOnce(Context) -> _Output` trait bound are intentionally included.
    /// They act as a hint to help the compiler infer closure types.
    #[inline]
    pub fn new<M: Mode, F, _Output>(context: Context, f: F) -> Self
    where
        F: FnOnce(Context) -> _Output,
        F: Infer<M, Context, Action = A>,
    {
        Self::assemble(context, f.infer())
    }

    /// Creates a guard from an already constructed guard action.
    ///
    /// This is the most direct constructor and is mainly used by macro internals
    /// and advanced integrations.
    #[inline]
    #[doc(hidden)]
    pub fn assemble(context: Context, action: A) -> Self {
        Self {
            inner: ManuallyDrop::new(ContextGuardInner { context, action }),
        }
    }
    /// Disarms the guard and returns both the context and the guard action.
    ///
    /// Unlike [`defuse`](Self::defuse), which drops the guard action, this
    /// method returns both parts, allowing reuse or re-wrapping.
    #[inline]
    pub fn disassemble(self) -> (Context, A) {
        let mut this = ManuallyDrop::new(self);
        unsafe {
            let ContextGuardInner { context, action } = ManuallyDrop::take(&mut this.inner);
            (context, action)
        }
    }

    /// Defuses the guard and returns the owned context without executing it.
    ///
    /// Use this when cleanup should be canceled and captured state should be
    /// recovered by the caller.
    #[inline]
    pub fn defuse(self) -> Context {
        let (context, _) = self.disassemble();
        context
    }

    /// Executes the guard body immediately and consumes the guard.
    ///
    /// This bypasses drop-based execution by running the closure eagerly.
    #[inline]
    pub fn trigger(self) -> A::Output {
        let (context, action) = self.disassemble();
        action.fire(context)
    }

    /// Transforms the guard's action using the provided closure.
    ///
    /// This is useful for wrapping or modifying the guard's execution logic while
    /// preserving the same context.
    #[inline]
    pub fn map<B: Action<Context>>(self, f: impl FnOnce(A) -> B) -> ContextGuard<Context, B> {
        let (context, action) = self.disassemble();
        ContextGuard::assemble(context, f(action))
    }
}

impl<Context, Spawner: TaskSpawner<Task>, A, Task> ContextGuard<Context, Spawn<Spawner, A, Task>>
where
    A: Action<Context, Output = Task>,
{
    /// Creates an async guard with a custom task spawner.
    ///
    /// The returned guard executes inline first and detaches through `spawner`
    /// if it is dropped before completion.
    #[inline]
    pub fn with_spawner(context: Context, spawner: Spawner, action: A) -> Self {
        Self::assemble(
            context,
            Spawn {
                spawner: Some(spawner),
                state: action::ActionState::Armed(action),
            },
        )
    }
}

impl<Context, A: Action<Context>> Drop for ContextGuard<Context, A> {
    /// Executes the guard closure.
    ///
    /// # Panics
    ///
    /// If the guard's closure panics while the current thread is already unwinding from a
    /// previous panic, Rust will trigger a double-panic and immediately **abort the process**.
    /// This is the standard behavior for `Drop` implementations in Rust.
    ///
    /// ```rust
    /// use std::process::Command;
    /// use std::env;
    ///
    /// // We run the abort in a subprocess so it doesn't fail the test suite.
    /// if env::var("RUN_ABORT_TEST").is_ok() {
    ///     struct PanicOnDrop;
    ///     impl Drop for PanicOnDrop {
    ///         fn drop(&mut self) {
    ///             panic!("thread panic"); // 1. Starts unwinding
    ///         }
    ///     }
    ///     let _trigger_unwind = PanicOnDrop;
    ///     
    ///     // 2. Guard panics during unwind -> process abort
    ///     let _g = guarden::guard!(sync [] { panic!("guard panic"); });
    ///     return;
    /// }
    ///
    /// let status = Command::new(env::current_exe().unwrap())
    ///     .env("RUN_ABORT_TEST", "1")
    ///     .status()
    ///     .unwrap();
    ///
    /// // Process should have aborted (non-zero exit status).
    /// assert!(!status.success());
    /// ```
    ///
    /// Since `guarden` is often used for critical cleanup operations, if process aborts are
    /// unacceptable for your server architecture, you must ensure that your guard closures
    /// do not contain diverging operations (like `panic!`, `unwrap()`, or `expect()`) that
    /// could fail.
    #[inline]
    fn drop(&mut self) {
        unsafe {
            let ContextGuardInner { context, action } = ManuallyDrop::take(&mut self.inner);
            action.fire(context);
        }
    }
}

/// A helper function for macros to enforce `FnOnce` bounds on closures.
#[doc(hidden)]
#[inline(always)]
pub fn __fn<V, F, R>(value: (V, F)) -> (V, F)
where
    F: FnOnce(V) -> R,
{
    value
}

#[cfg(all(test, feature = "tokio"))]
mod tests {
    use super::*;

    use alloc::sync::Arc;
    use core::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    #[test]
    fn sync_disassemble() {
        let state = Arc::new(AtomicUsize::new(0));
        let guard = ContextGuard::new(state.clone(), |s| {
            s.fetch_add(1, Ordering::SeqCst);
        });

        // Disassemble the guard
        let (context, action) = guard.disassemble();

        // State should not have changed
        assert_eq!(state.load(Ordering::SeqCst), 0);

        // We can manually fire the action later
        action.fire(context);
        assert_eq!(state.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn sync_defuse() {
        let state = Arc::new(AtomicUsize::new(0));
        let guard = ContextGuard::new(state.clone(), |s| {
            s.fetch_add(1, Ordering::SeqCst);
        });

        let context = guard.defuse();

        // State should not have changed, and action is dropped
        assert_eq!(state.load(Ordering::SeqCst), 0);

        // We successfully recovered the context
        assert_eq!(context.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn sync_trigger() {
        let state = Arc::new(AtomicUsize::new(0));
        let guard = ContextGuard::assemble(state.clone(), |s: Arc<AtomicUsize>| {
            s.fetch_add(1, Ordering::SeqCst);
            "result"
        });

        // Trigger should return the action's output
        let res = guard.trigger();

        assert_eq!(res, "result");
        assert_eq!(state.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn sync_map() {
        let state = Arc::new(AtomicUsize::new(0));

        let guard = ContextGuard::new(state.clone(), |s| {
            s.fetch_add(1, Ordering::SeqCst);
        });

        // Map the action to a new closure that does something else
        let mapped_guard = guard.map(|original_action| {
            // We use action::closure instead of implementing Action manually
            move |s: Arc<AtomicUsize>| {
                // Do something before
                s.fetch_add(10, Ordering::SeqCst);
                // Execute original
                original_action.fire(s.clone());
                // Do something after
                s.fetch_add(100, Ordering::SeqCst);
            }
        });

        mapped_guard.trigger();

        // 10 + 1 + 100 = 111
        assert_eq!(state.load(Ordering::SeqCst), 111);
    }

    #[tokio::test]
    async fn async_disassemble() {
        let state = Arc::new(AtomicUsize::new(0));
        let guard = ContextGuard::new(state.clone(), |s| async move {
            s.fetch_add(1, Ordering::SeqCst);
        });

        let (context, action) = guard.disassemble();
        assert_eq!(state.load(Ordering::SeqCst), 0);

        action.fire(context).await;
        assert_eq!(state.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn async_defuse() {
        let state = Arc::new(AtomicUsize::new(0));
        let guard = ContextGuard::new(state.clone(), |s| async move {
            s.fetch_add(1, Ordering::SeqCst);
        });

        let _context = guard.defuse();

        // Sleep to ensure no detached task runs
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(state.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn async_trigger() {
        let state = Arc::new(AtomicUsize::new(0));
        let guard = ContextGuard::new(state.clone(), |s| async move {
            s.fetch_add(1, Ordering::SeqCst);
            "async_result"
        });

        let res = guard.trigger().await;

        assert_eq!(res, "async_result");
        assert_eq!(state.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn async_map() {
        let state = Arc::new(AtomicUsize::new(0));

        let guard = ContextGuard::new(state.clone(), |s| async move {
            s.fetch_add(1, Ordering::SeqCst);
        });

        let mapped_guard = guard.map(|original_action| {
            move |s: Arc<AtomicUsize>| async move {
                s.fetch_add(10, Ordering::SeqCst);
                original_action.fire(s.clone()).await;
                s.fetch_add(100, Ordering::SeqCst);
            }
        });

        mapped_guard.trigger().await;
        assert_eq!(state.load(Ordering::SeqCst), 111);
    }
}
