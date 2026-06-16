use crate::Guard;
use crate::guard::action::{Action, ActionState, Spawn};
use crate::guard::{ContextGuard, GuardExt};
use crate::task::{BoxTask, DetachableTask, TaskSpawner};
use alloc::boxed::Box;
use core::future::Future;
use core::pin::Pin;
use std::ops::{Deref, DerefMut};

/// A trait for guards that can be boxed into a type-erased form.
pub trait ActionBoxExt<Context> {
    type Boxed: Action<Context>;
    fn boxed(self) -> Self::Boxed;
}

impl<Context, A> ContextGuard<Context, A>
where
    A: Action<Context>,
{
    /// Boxes the guard, erasing the closure type.
    ///
    /// For synchronous guards, this produces a [`BoxSyncGuard`] with a single allocation.
    /// For asynchronous guards, this produces a [`BoxAsyncGuard`] that reuses a single
    /// allocation for both the closure and the resulting future.
    #[inline]
    pub fn boxed<Output>(self) -> BoxContextGuard<Context, Output>
    where
        A: ActionBoxExt<Context, Boxed = BoxAction<Context, Output>>,
    {
        BoxContextGuard {
            guard: self.map(ActionBoxExt::boxed),
        }
    }
}

/// Object-safe interface for a type-erased guard action.
///
/// For synchronous guards, the boxed value holds the closure.
/// For asynchronous guards, the boxed value transitions in place from holding the closure
/// to holding the produced future when fired. This in-place transition is what keeps
/// [`ContextGuard::boxed`] on the async path down to a single heap allocation.
pub trait BoxedAction<Context> {
    type Output;
    fn fire(self: Box<Self>, context: Context) -> Self::Output;
}

// Blanket Boxing for any type that implements BoxAction
impl<Context, A> ActionBoxExt<Context> for A
where
    A: BoxedAction<Context> + 'static,
{
    type Boxed = BoxAction<Context, A::Output>;

    #[inline]
    fn boxed(self) -> Self::Boxed {
        Box::new(self)
    }
}

/// A boxed, type-erased action.
type BoxAction<Context, Output> = Box<dyn BoxedAction<Context, Output = Output>>;

impl<Context, Output> Action<Context> for BoxAction<Context, Output> {
    type Output = Output;
    #[inline]
    fn fire(self, context: Context) -> Self::Output {
        self.fire(context)
    }
}

#[repr(transparent)]
#[must_use = "if you don't bind a guard to a variable, its action executes immediately (e.g., `let _g = guard!(...);`)"]
pub struct BoxContextGuard<Context, Output> {
    guard: ContextGuard<Context, BoxAction<Context, Output>>,
}

impl<Context, Output> Guard for BoxContextGuard<Context, Output> {
    type Context = Context;
    type Action = BoxAction<Context, Output>;

    #[inline]
    fn disassemble(self) -> (Self::Context, Self::Action) {
        self.guard.disassemble()
    }
}

impl<Context, Output> Deref for BoxContextGuard<Context, Output> {
    type Target = ContextGuard<Context, BoxAction<Context, Output>>;
    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.guard
    }
}

impl<Context, Output> DerefMut for BoxContextGuard<Context, Output> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.guard
    }
}

// region Sync

// Specific BoxAction implementation for synchronous actions
impl<Context, F, Output> BoxedAction<Context> for F
where
    F: FnOnce(Context) -> Output,
{
    type Output = Output;

    #[inline]
    fn fire(self: Box<Self>, context: Context) -> Self::Output {
        (*self)(context)
    }
}

/// A boxed, type-erased synchronous guard.
///
/// Boxing erases the closure type, allowing guards to be stored in structs
/// and collections. Use [`ContextGuard::boxed`] on a sync guard to obtain
/// this type.
pub type BoxSyncGuard<Context, Output = ()> = BoxContextGuard<Context, Output>;

// endregion

// region Async

/// An object-safe interface for a type-erased pinned task that can be spawned.
pub trait BoxedTask: Future {
    fn spawn(self: BoxTask<Self>);
}

impl<Spawner: TaskSpawner<Self>, A, Task: Future> BoxedTask for Spawn<Spawner, A, Task> {
    fn spawn(mut self: BoxTask<Self>) {
        unsafe { self.as_mut().get_unchecked_mut() }
            .spawner
            .take()
            .expect("task spawned multiple times")
            .spawn(self);
    }
}

/// A zero-sized type that dispatches spawn calls to a dynamically erased task.
#[derive(Debug, Default, Clone, Copy)]
pub struct BoxedSpawner;

impl<Output> TaskSpawner<dyn BoxedTask<Output = Output>> for BoxedSpawner {
    type Output = ();

    #[inline]
    fn spawn(self, task: BoxTask<dyn BoxedTask<Output = Output>>) -> Self::Output {
        task.spawn()
    }
}

impl<Context, Spawner, A, Task> BoxedAction<Context> for Spawn<Spawner, A, Task>
where
    A: Action<Context, Output = Task> + 'static,
    Task: Future + 'static,
    Spawner: TaskSpawner<Self> + 'static,
{
    type Output = DetachableTask<BoxedSpawner, dyn BoxedTask<Output = Task::Output>>;

    fn fire(mut self: Box<Self>, context: Context) -> Self::Output {
        let fut = self.ignite(context);
        self.state = ActionState::Fired(fut);

        DetachableTask::from_boxed(BoxedSpawner, Pin::from(self))
    }
}

/// A boxed, type-erased asynchronous guard.
///
/// The guard body and the future it produces share a single heap allocation:
/// the boxed value transitions in place from holding the body to holding the
/// future when fired. Use [`ContextGuard::boxed`] on an async guard to obtain
/// this type.
pub type BoxAsyncGuard<Context, Output = ()> =
    BoxContextGuard<Context, DetachableTask<BoxedSpawner, dyn BoxedTask<Output = Output>>>;

// endregion

#[cfg(all(test, feature = "tokio"))]
mod tests {
    use crate::{guard, guard::Guard};
    use alloc::sync::Arc;
    use core::sync::atomic::{AtomicUsize, Ordering};
    use core::time::Duration;

    #[test]
    fn sync_boxed_drop() {
        let fired = Arc::new(AtomicUsize::new(0));
        {
            let guard = guard!(sync [f = fired.clone()] {
                f.store(1, Ordering::SeqCst);
            });
            let _boxed = guard.boxed();
            assert_eq!(fired.load(Ordering::SeqCst), 0);
        } // Boxed guard dropped
        assert_eq!(fired.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn sync_boxed_trigger() {
        let fired = Arc::new(AtomicUsize::new(0));
        let guard = guard!(sync [f = fired.clone()] {
            f.store(2, Ordering::SeqCst);
            42
        });
        let boxed = guard.boxed();

        let result = boxed.trigger();
        assert_eq!(fired.load(Ordering::SeqCst), 2);
        assert_eq!(result, 42);
    }

    #[test]
    fn sync_boxed_defuse() {
        let fired = Arc::new(AtomicUsize::new(0));
        let guard = guard!(sync [f = fired.clone(), _val = 100usize] {
            f.store(3, Ordering::SeqCst);
        });
        let boxed = guard.boxed();

        let ctx = boxed.defuse();
        assert_eq!(fired.load(Ordering::SeqCst), 0); // Never executed
        assert_eq!(ctx.1, 100); // Context recovered
    }

    #[test]
    fn sync_boxed_deref_mut() {
        let guard = guard!(sync [mut a = 5usize, b = 10usize] {
            assert_eq!(a, 100);
            assert_eq!(b, 10);
        });
        let mut boxed = guard.boxed();

        // Test Deref and DerefMut (modifying the inner context of the boxed guard)
        boxed.0 = 100;

        boxed.trigger(); // Consumes and asserts internally
    }

    #[tokio::test]
    async fn async_boxed_drop() {
        let fired = Arc::new(AtomicUsize::new(0));
        {
            let guard = guard!([f = fired.clone()] async move {
                f.store(1, Ordering::SeqCst);
            });
            let _boxed = guard.boxed();
        } // Boxed guard dropped, spawning background task

        // Wait for the background task to finish
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(fired.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn async_boxed_trigger() {
        let fired = Arc::new(AtomicUsize::new(0));
        let guard = guard!([f = fired.clone()] async move {
            f.store(2, Ordering::SeqCst);
            42
        });
        let boxed = guard.boxed();

        let result = boxed.trigger().await; // boxed.trigger() returns a DetachableTask, which implements Future
        assert_eq!(fired.load(Ordering::SeqCst), 2);
        assert_eq!(result, 42);
    }

    #[tokio::test]
    async fn async_boxed_defuse() {
        let fired = Arc::new(AtomicUsize::new(0));
        let guard = guard!([f = fired.clone(), _val = 99usize] async move {
            f.store(3, Ordering::SeqCst);
        });
        let boxed = guard.boxed();

        let ctx = boxed.defuse();
        assert_eq!(fired.load(Ordering::SeqCst), 0); // Never executed
        assert_eq!(ctx.1, 99); // Context recovered

        // Ensure no ghost tasks were spawned
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(fired.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn async_boxed_deref_mut() {
        let fired = Arc::new(AtomicUsize::new(0));
        let guard = guard!([f = fired.clone(), mut val = 0usize] async move {
            f.store(val, Ordering::SeqCst);
        });
        let mut boxed = guard.boxed();

        // Test DerefMut on async boxed guard
        boxed.1 = 999;

        boxed.trigger().await;
        assert_eq!(fired.load(Ordering::SeqCst), 999);
    }
}
