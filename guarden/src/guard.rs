use std::fmt::Debug;
use std::mem::ManuallyDrop;
use std::ops::{Deref, DerefMut};

pub trait GuardMode {
    const DEBUG: &'static str;
    type Spawner;
    fn spawner() -> Self::Spawner;
}

pub struct SyncMode;

impl GuardMode for SyncMode {
    const DEBUG: &'static str = "ContextGuard::Sync";
    type Spawner = ();
    fn spawner() -> Self::Spawner {}
}

pub trait CallableGuard<Mode: GuardMode, Context> {
    type Output;
    fn call(self, context: Context, spawner: Mode::Spawner) -> Self::Output;
}

impl<Context, Guard> CallableGuard<SyncMode, Context> for Guard
where
    Guard: FnOnce(Context),
{
    type Output = ();

    fn call(self, context: Context, _spawner: ()) -> Self::Output {
        self(context)
    }
}

cfg_select! {
    feature = "tokio" => {
        use crate::task::{DetachableTask};
        use tokio::runtime::Handle;

        pub struct AsyncMode;

        impl GuardMode for AsyncMode {
            const DEBUG: &'static str = "ContextGuard::Async";
            type Spawner = Handle;
            fn spawner() -> Self::Spawner {
                Handle::current()
            }
        }

        impl<Context, Guard, Task, _R> CallableGuard<AsyncMode, Context> for Guard
        where
            Guard: FnOnce(Context) -> Task,
            Task: Future<Output = _R> + Send + 'static,
            _R: Send + 'static,
        {
            type Output = DetachableTask<Handle, Task>;

            fn call(self, context: Context, spawner: Handle) -> Self::Output {
                DetachableTask::with_spawner(spawner, self(context))
            }
        }
    }
}

struct ContextGuardInner<Mode: GuardMode, Context, Guard: CallableGuard<Mode, Context>> {
    context: Context,
    guard: Guard,
    spawner: Mode::Spawner,
}

pub struct ContextGuard<Mode: GuardMode, Context, Guard: CallableGuard<Mode, Context>> {
    inner: ManuallyDrop<ContextGuardInner<Mode, Context, Guard>>,
}

impl<Mode: GuardMode, Context, Guard: CallableGuard<Mode, Context>> Deref
    for ContextGuard<Mode, Context, Guard>
{
    type Target = Context;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.inner.context
    }
}

impl<Mode: GuardMode, Context, Guard: CallableGuard<Mode, Context>> DerefMut
    for ContextGuard<Mode, Context, Guard>
{
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner.context
    }
}

impl<Mode: GuardMode, Context: Debug, Guard: CallableGuard<Mode, Context>> Debug
    for ContextGuard<Mode, Context, Guard>
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct(Mode::DEBUG)
            .field("context", &self.inner.context)
            .finish_non_exhaustive()
    }
}

impl<Mode: GuardMode, Context, Guard: CallableGuard<Mode, Context>>
    ContextGuard<Mode, Context, Guard>
{
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
        ContextGuard {
            inner: ManuallyDrop::new(ContextGuardInner {
                context,
                guard,
                spawner: Mode::spawner(),
            }),
        }
    }
}

impl<Mode: GuardMode, Context, Guard: CallableGuard<Mode, Context>>
    ContextGuard<Mode, Context, Guard>
{
    unsafe fn call(&mut self) -> Guard::Output {
        unsafe {
            let ContextGuardInner {
                context,
                guard,
                spawner,
            } = ManuallyDrop::take(&mut self.inner);

            guard.call(context, spawner)
        }
    }

    pub fn trigger(self) -> Guard::Output {
        let mut this = ManuallyDrop::new(self);
        unsafe { this.call() }
    }

    pub fn defuse(self) -> Context {
        let mut this = ManuallyDrop::new(self);
        unsafe {
            let ContextGuardInner {
                context,
                guard: _guard,
                spawner: _spawner,
            } = ManuallyDrop::take(&mut this.inner);
            context
        }
    }
}

impl<Mode: GuardMode, Context, Guard: CallableGuard<Mode, Context>> Drop
    for ContextGuard<Mode, Context, Guard>
{
    fn drop(&mut self) {
        let _: Guard::Output = unsafe { self.call() };
    }
}
