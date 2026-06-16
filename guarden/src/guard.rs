pub mod action;
pub mod boxed;

use crate::task::TaskSpawner;
use action::mode::Mode;
use action::{Action, Infer, Spawn};
use core::fmt;
use core::fmt::Debug;
use core::mem::ManuallyDrop;
use core::ops::{Deref, DerefMut};

/// Common operations available on all context guards.
pub trait Guard: Sized {
    type Context;
    type Action: Action<Self::Context>;

    /// Disarms the guard and returns both the context and the guard action.
    ///
    /// Unlike [`defuse`](Self::defuse), which drops the guard action, this
    /// method returns both parts, allowing reuse or re-wrapping.
    fn disassemble(self) -> (Self::Context, Self::Action);

    /// Defuses the guard and returns the owned context without executing it.
    ///
    /// Use this when cleanup should be canceled and captured state should be
    /// recovered by the caller.
    #[inline]
    fn defuse(self) -> Self::Context {
        let (context, _) = self.disassemble();
        context
    }

    /// Executes the guard body immediately and consumes the guard.
    ///
    /// This bypasses drop-based execution by running the closure eagerly.
    #[inline]
    fn trigger(self) -> <Self::Action as Action<Self::Context>>::Output {
        let (context, action) = self.disassemble();
        action.fire(context)
    }
}

/// Extension trait for transforming guard actions.
pub trait GuardExt: Guard {
    /// The guard type returned when its action is transformed.
    type Mapped<B: Action<Self::Context>>: Guard<Context = Self::Context, Action = B>;

    /// Transforms the guard's action using the provided closure.
    ///
    /// This is useful for wrapping or modifying the guard's execution logic while
    /// preserving the same context.
    fn map<B, F>(self, f: F) -> Self::Mapped<B>
    where
        B: Action<Self::Context>,
        F: FnOnce(Self::Action) -> B;
}

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
}

impl<Context, A: Action<Context>> Guard for ContextGuard<Context, A> {
    type Context = Context;
    type Action = A;

    #[inline]
    fn disassemble(self) -> (Self::Context, Self::Action) {
        let mut this = ManuallyDrop::new(self);
        unsafe {
            let ContextGuardInner { context, action } = ManuallyDrop::take(&mut this.inner);
            (context, action)
        }
    }
}

impl<Context, A: Action<Context>> GuardExt for ContextGuard<Context, A> {
    type Mapped<B: Action<Context>> = ContextGuard<Context, B>;

    #[inline]
    fn map<B, F>(self, f: F) -> Self::Mapped<B>
    where
        B: Action<Context>,
        F: FnOnce(Self::Action) -> B,
    {
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
