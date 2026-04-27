use crate::task::{DetachableTask, TaskSpawner};
use std::fmt::Debug;
use std::mem::ManuallyDrop;
use std::ops::{Deref, DerefMut};

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

        /// **Note on `Handle::current()`**: The Tokio runtime handle is acquired lazily when the
        /// guard is triggered or dropped, rather than when it is created. This allows you to
        /// create the guard in a non-Tokio thread (e.g., during server bootstrapping or inside a
        /// builder pattern) as long as the guard is ultimately dropped or triggered within a valid
        /// Tokio context. If the guard is dropped outside a Tokio context, it will panic.
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
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
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
    #[inline]
    pub fn with_spawner(spawner: Spawner, context: Context, guard: Guard) -> Self {
        Self::with_guard(context, AsyncGuard { spawner, guard })
    }
}

impl<const SYNC: bool, const ASYNC: bool, Context, Guard: CallableGuard<SYNC, ASYNC, Context>>
    ContextGuard<SYNC, ASYNC, Context, Guard>
{
    #[inline]
    unsafe fn call(&mut self) -> Guard::Output {
        unsafe {
            let ContextGuardInner { context, guard } = ManuallyDrop::take(&mut self.0);
            guard.call(context)
        }
    }

    #[inline]
    pub fn trigger(self) -> Guard::Output {
        let mut this = ManuallyDrop::new(self);
        unsafe { this.call() }
    }

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
