use std::fmt::Debug;
use std::mem::ManuallyDrop;
use std::ops::{Deref, DerefMut};

pub trait CallableGuard<const ASYNC: bool, Context> {
    type Output;
    fn call(self, context: Context) -> Self::Output;
}

impl<Context, Guard> CallableGuard<false, Context> for Guard
where
    Guard: FnOnce(Context),
{
    type Output = ();

    fn call(self, context: Context) -> Self::Output {
        self(context)
    }
}

cfg_select! {
    feature = "tokio" => {
        use crate::task::{DetachableTask};
        use tokio::runtime::Handle;

        /// Mode for asynchronous guards that detach a task upon dropping.
        ///
        /// **Note on `Handle::current()`**: The Tokio runtime handle is acquired lazily when the
        /// guard is triggered or dropped, rather than when it is created. This allows you to
        /// create the guard in a non-Tokio thread (e.g., during server bootstrapping or inside a
        /// builder pattern) as long as the guard is ultimately dropped or triggered within a valid
        /// Tokio context. If the guard is dropped outside a Tokio context, it will panic.
        pub struct AsyncMode;

        impl<Context, Guard, Task, _R> CallableGuard<true, Context> for Guard
        where
            Guard: FnOnce(Context) -> Task,
            Task: Future<Output = _R> + Send + 'static,
            _R: Send + 'static,
        {
            type Output = DetachableTask<Handle, Task>;

            fn call(self, context: Context) -> Self::Output {
                DetachableTask::new(self(context))
            }
        }
    }
}

pub struct ContextGuard<const ASYNC: bool, Context, Guard: CallableGuard<ASYNC, Context>> {
    context: ManuallyDrop<Context>,
    guard: ManuallyDrop<Guard>,
}

impl<const ASYNC: bool, Context, Guard: CallableGuard<ASYNC, Context>> Deref
    for ContextGuard<ASYNC, Context, Guard>
{
    type Target = Context;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.context
    }
}

impl<const ASYNC: bool, Context, Guard: CallableGuard<ASYNC, Context>> DerefMut
    for ContextGuard<ASYNC, Context, Guard>
{
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.context
    }
}

impl<const ASYNC: bool, Context: Debug, Guard: CallableGuard<ASYNC, Context>> Debug
    for ContextGuard<ASYNC, Context, Guard>
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = if ASYNC {
            "ContextGuard::Async"
        } else {
            "ContextGuard::Sync"
        };
        f.debug_struct(name)
            .field("context", &self.context)
            .finish_non_exhaustive()
    }
}

impl<const ASYNC: bool, Context, Guard: CallableGuard<ASYNC, Context>>
    ContextGuard<ASYNC, Context, Guard>
{
    #[inline]
    pub fn with_guard(context: Context, guard: Guard) -> Self {
        ContextGuard {
            context: ManuallyDrop::new(context),
            guard: ManuallyDrop::new(guard),
        }
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

impl<const ASYNC: bool, Context, Guard: CallableGuard<ASYNC, Context>>
    ContextGuard<ASYNC, Context, Guard>
{
    unsafe fn call(&mut self) -> Guard::Output {
        unsafe {
            let context = ManuallyDrop::take(&mut self.context);
            let guard = ManuallyDrop::take(&mut self.guard);

            guard.call(context)
        }
    }

    pub fn trigger(self) -> Guard::Output {
        let mut this = ManuallyDrop::new(self);
        unsafe { this.call() }
    }

    pub fn defuse(self) -> Context {
        let mut this = ManuallyDrop::new(self);
        unsafe {
            let context = ManuallyDrop::take(&mut this.context);
            ManuallyDrop::drop(&mut this.guard);
            context
        }
    }
}

impl<const ASYNC: bool, Context, Guard: CallableGuard<ASYNC, Context>> Drop
    for ContextGuard<ASYNC, Context, Guard>
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
    fn drop(&mut self) {
        let _ = unsafe { self.call() };
    }
}
