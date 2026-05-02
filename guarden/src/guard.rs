use crate::task::{DetachableTask, TaskSpawner};
use core::fmt;
use core::fmt::Debug;
use core::mem::ManuallyDrop;
use core::ops::{Deref, DerefMut};

/// A callable guard body used by [`ContextGuard`].
///
/// The `SYNC`/`ASYNC` const parameters encode how the guard body should be
/// interpreted by macro expansion:
/// - `SYNC = true` forces synchronous execution.
/// - `SYNC = false` enables inference mode, where the compiler determines
///   whether async wrapping is needed from the closure return type.
/// - In inference mode, `ASYNC` is effectively inferred: future-returning
///   closures use async detachment, while non-future closures stay sync.
pub trait CallableGuard<const SYNC: bool, const ASYNC: bool, Context> {
    type Output;
    fn call(self, context: Context) -> Self::Output;
}

// SYNC = false, ASYNC = false (Inferred Sync)
impl<Context, Guard> CallableGuard<false, false, Context> for Guard
where
    Guard: FnOnce(Context),
{
    type Output = ();

    #[inline]
    fn call(self, context: Context) -> Self::Output {
        self(context)
    }
}

// SYNC = true, ASYNC = _ (Explicit Sync)
impl<const ASYNC: bool, Context, Guard, R> CallableGuard<true, ASYNC, Context> for Guard
where
    Guard: FnOnce(Context) -> R,
{
    type Output = R;

    #[inline]
    fn call(self, context: Context) -> Self::Output {
        self(context)
    }
}

/// Adapter that turns an async guard closure into a [`DetachableTask`].
pub struct AsyncGuard<Spawner, Guard> {
    spawner: Spawner,
    guard: Guard,
}

impl<Context, Spawner: TaskSpawner<Task>, Guard, Task> CallableGuard<false, true, Context>
    for AsyncGuard<Spawner, Guard>
where
    Guard: FnOnce(Context) -> Task,
{
    type Output = DetachableTask<Spawner, Task>;

    #[inline]
    fn call(self, context: Context) -> Self::Output {
        DetachableTask::with_spawner(self.spawner, (self.guard)(context))
    }
}

cfg_select! {
    feature = "tokio" => {
        use crate::task::TokioHandle;

        /// **Note on `Handle::current()`**: The Tokio runtime handle is acquired lazily at
        /// **detachment time**, never when the guard is constructed. This allows you to
        /// create the guard in a non-Tokio thread (e.g., during server bootstrapping or inside a
        /// builder pattern) as long as detachment eventually happens within a valid Tokio context.
        /// In practice, this happens when an async guard is dropped while pending. If detachment
        /// occurs outside a Tokio context, it will panic.
        type DefaultAsyncSpawner = TokioHandle;
        const DEFAULT_ASYNC_SPAWNER: DefaultAsyncSpawner = TokioHandle;
    }

    _ => {
        type DefaultAsyncSpawner = ();
        const DEFAULT_ASYNC_SPAWNER: DefaultAsyncSpawner = ();
    }
}

impl<Context, Guard, Task: Future> CallableGuard<false, true, Context> for Guard
where
    Guard: FnOnce(Context) -> Task,
    DefaultAsyncSpawner: TaskSpawner<Task>,
{
    type Output =
        <AsyncGuard<DefaultAsyncSpawner, Guard> as CallableGuard<false, true, Context>>::Output;

    #[inline]
    fn call(self, context: Context) -> Self::Output {
        AsyncGuard {
            spawner: DEFAULT_ASYNC_SPAWNER,
            guard: self,
        }
        .call(context)
    }
}

struct ContextGuardInner<
    const SYNC: bool,
    const ASYNC: bool,
    Context,
    Guard: CallableGuard<SYNC, ASYNC, Context>,
> {
    context: Context,
    guard: Guard,
}

/// RAII guard that owns a context value and executes a closure on drop.
///
/// You can either:
/// - let `Drop` trigger the guard body automatically,
/// - call [`trigger`](Self::trigger) for eager execution, or
/// - call [`defuse`](Self::defuse) to recover the context without execution.
pub struct ContextGuard<
    const SYNC: bool,
    const ASYNC: bool,
    Context,
    Guard: CallableGuard<SYNC, ASYNC, Context>,
>(ManuallyDrop<ContextGuardInner<SYNC, ASYNC, Context, Guard>>);

impl<const SYNC: bool, const ASYNC: bool, Context, Guard: CallableGuard<SYNC, ASYNC, Context>> Deref
    for ContextGuard<SYNC, ASYNC, Context, Guard>
{
    type Target = Context;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.0.context
    }
}

impl<const SYNC: bool, const ASYNC: bool, Context, Guard: CallableGuard<SYNC, ASYNC, Context>>
    DerefMut for ContextGuard<SYNC, ASYNC, Context, Guard>
{
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0.context
    }
}

impl<
    const SYNC: bool,
    const ASYNC: bool,
    Context: Debug,
    Guard: CallableGuard<SYNC, ASYNC, Context>,
> Debug for ContextGuard<SYNC, ASYNC, Context, Guard>
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = if !SYNC && ASYNC {
            "ContextGuard::Async"
        } else {
            "ContextGuard::Sync"
        };
        f.debug_struct(name)
            .field("context", &self.0.context)
            .finish_non_exhaustive()
    }
}

impl<const SYNC: bool, const ASYNC: bool, Context, Guard: CallableGuard<SYNC, ASYNC, Context>>
    ContextGuard<SYNC, ASYNC, Context, Guard>
{
    /// Creates a guard from an already constructed callable guard type.
    ///
    /// This is the most direct constructor and is mainly used by macro internals
    /// and advanced integrations.
    #[inline]
    pub fn with_guard(context: Context, guard: Guard) -> Self {
        Self(ManuallyDrop::new(ContextGuardInner { context, guard }))
    }

    /// Creates a new `ContextGuard`.
    ///
    /// **Note on generics:** The seemingly unused `_R` generic parameter and the
    /// `Guard: FnOnce(Context) -> _R` trait bound are intentionally included.
    /// They act as a hint to help the compiler infer closure types.
    #[inline]
    pub fn new<_R>(context: Context, guard: Guard) -> Self
    where
        Guard: FnOnce(Context) -> _R,
    {
        Self::with_guard(context, guard)
    }
}

impl<Context, Spawner: TaskSpawner<Task>, Guard, Task>
    ContextGuard<false, true, Context, AsyncGuard<Spawner, Guard>>
where
    Guard: FnOnce(Context) -> Task,
{
    /// Creates an async guard with a custom task spawner.
    ///
    /// The returned guard executes inline first and detaches through `spawner`
    /// if it is dropped before completion.
    #[inline]
    pub fn with_spawner(spawner: Spawner, context: Context, guard: Guard) -> Self {
        Self::with_guard(context, AsyncGuard { spawner, guard })
    }
}

impl<const SYNC: bool, const ASYNC: bool, Context, Guard: CallableGuard<SYNC, ASYNC, Context>>
    ContextGuard<SYNC, ASYNC, Context, Guard>
{
    // SAFETY: callers must guarantee this guard has not been consumed before.
    #[inline]
    unsafe fn call(&mut self) -> Guard::Output {
        unsafe {
            let ContextGuardInner { context, guard } = ManuallyDrop::take(&mut self.0);
            guard.call(context)
        }
    }

    /// Executes the guard body immediately and consumes the guard.
    ///
    /// This bypasses drop-based execution by running the closure eagerly.
    #[inline]
    pub fn trigger(self) -> Guard::Output {
        let mut this = ManuallyDrop::new(self);
        unsafe { this.call() }
    }

    /// Defuses the guard and returns the owned context without executing it.
    ///
    /// Use this when cleanup should be canceled and captured state should be
    /// recovered by the caller.
    #[inline]
    pub fn defuse(self) -> Context {
        let mut this = ManuallyDrop::new(self);
        unsafe {
            let ContextGuardInner { context, guard: _ } = ManuallyDrop::take(&mut this.0);
            context
        }
    }
}

impl<const SYNC: bool, const ASYNC: bool, Context, Guard: CallableGuard<SYNC, ASYNC, Context>> Drop
    for ContextGuard<SYNC, ASYNC, Context, Guard>
{
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
        let _ = unsafe { self.call() };
    }
}
