use crate::guard::ContextGuard;
use crate::guard::Guard;
use crate::guard::action::Action;
use alloc::boxed::Box;
use core::fmt;
use core::fmt::Debug;
use core::future::Future;
use core::ops::DerefMut;
use core::pin::Pin;
use core::task::{Context, Poll};
use futures_core::future::FusedFuture;

/// A pinned, heap-allocated task.
///
/// **Why Box?** Heap allocation is required because if the task detaches,
/// it outlives the current stack frame. `Pin<Box<_>>` ensures its memory address
/// remains completely stable during and after the transfer.
pub type BoxTask<Task> = Pin<Box<Task>>;

// region DetachableTask

struct DetachableTaskContext<Spawner, Task: ?Sized> {
    spawner: Spawner,
    task: Option<BoxTask<Task>>,
}

struct DetachableTaskGuard;

impl<Spawner: TaskSpawner<Task>, Task: ?Sized> Action<DetachableTaskContext<Spawner, Task>>
    for DetachableTaskGuard
{
    type Output = ();

    #[inline]
    fn fire(self, context: DetachableTaskContext<Spawner, Task>) {
        if let Some(task) = context.task {
            context.spawner.spawn(task);
        }
    }
}

type DetachableTaskContextGuard<Spawner, Task> =
    ContextGuard<DetachableTaskContext<Spawner, Task>, DetachableTaskGuard>;

/// A task wrapper that executes inline but automatically detaches to a background spawner
/// if the current execution context is interrupted or dropped.
///
/// `DetachableTask` ensures anti-cancellation. If the outer future is dropped (e.g., due to
/// a timeout or a `select!` branch failing), the underlying unfinished task is seamlessly
/// transferred to a background executor via an RAII guard.
///
/// # Advantages over `tokio::spawn` + `.await JoinHandle`
///
/// 1. **Zero Initial Scheduling Overhead**: Prioritizes inline execution. If the task
///    completes before being interrupted, it entirely bypasses the runtime's scheduling queue,
///    eliminating queuing latency and context-switching CPU costs. Spawning is strictly a fallback.
///
/// 2. **Context Locality**: Before detachment, the task is polled directly by the caller's thread.
///    This implicitly preserves the current execution context, including thread-local storage (TLS),
///    which would otherwise be lost or require explicit propagation across task boundaries.
///
///    > **⚠️ WARNING on Detachment & Local State:**
///    > While the local state is preserved *during inline execution*, if the task yields (e.g. `await`)
///    > and is subsequently detached (via Drop), the remaining execution will be transferred to a
///    > newly spawned background task. At this point, **the caller's `task_local!` and TLS state
///    > will be silently lost**. Do not rely on implicit local state across `.await` points inside
///    > the guarded future.
#[must_use = "futures do nothing unless you `.await` or poll them; dropping this task will detach it to the background"]
pub struct DetachableTask<Spawner: TaskSpawner<Task>, Task: ?Sized> {
    guard: DetachableTaskContextGuard<Spawner, Task>,
}

impl<Spawner: TaskSpawner<Task>, Task: ?Sized> Debug for DetachableTask<Spawner, Task> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DetachableTask").finish_non_exhaustive()
    }
}

impl<Spawner: TaskSpawner<Task>, Task: ?Sized> DetachableTask<Spawner, Task> {
    /// Forces detachment immediately.
    ///
    /// If the inner task has not completed yet, it is handed to the configured
    /// [`TaskSpawner`].
    #[inline]
    pub fn detach(self) {
        self.guard.trigger()
    }

    /// Cancels detachment and returns the pinned task back to the caller.
    ///
    /// This is useful when you need to move execution ownership elsewhere
    /// manually.
    #[inline]
    pub fn reclaim(self) -> BoxTask<Task> {
        self.guard.defuse().task.unwrap()
    }
}

/// Spawns a detached task produced by [`DetachableTask`].
///
/// Implement this trait to integrate with a runtime or custom executor.
pub trait TaskSpawner<Task: ?Sized> {
    /// Return type of the spawn operation.
    type Output;

    /// Consumes `self` and schedules `task` for background execution.
    fn spawn(self, task: BoxTask<Task>) -> Self::Output
    where
        Self: Sized;
}

impl<F, Output, Task: ?Sized> TaskSpawner<Task> for F
where
    F: FnOnce(BoxTask<Task>) -> Output,
{
    type Output = Output;

    #[inline]
    fn spawn(self, task: BoxTask<Task>) -> Self::Output {
        self(task)
    }
}

impl<Task: ?Sized> TaskSpawner<Task> for () {
    type Output = BoxTask<Task>;

    #[inline]
    fn spawn(self, task: BoxTask<Task>) -> Self::Output {
        task
    }
}

cfg_select! {
    feature = "tokio" => {
        use tokio::runtime::Handle;
        use tokio::task::JoinHandle;

        /// Tokio-backed spawner that uses [`Handle::current`].
        ///
        /// The runtime handle is resolved only when a task is detached/spawned,
        /// not when a [`DetachableTask`] is constructed. Calling detach/spawn
        /// outside a Tokio runtime will panic.
        #[derive(Debug, Default, Clone, Copy)]
        pub struct TokioHandle;

        impl<Task> TaskSpawner<Task> for TokioHandle
        where
            Task: ?Sized + Future + Send + 'static,
            <Task as Future>::Output: Send + 'static,
        {
            type Output = JoinHandle<<Task as Future>::Output>;

            #[inline]
            fn spawn(self, task: BoxTask<Task>) -> Self::Output {
                Handle::current().spawn(task)
            }
        }

        pub type DefaultSpawner = TokioHandle;
        pub const DEFAULT_SPAWNER: DefaultSpawner = TokioHandle;
    }

    _ => {
        pub type DefaultSpawner = ();
        pub const DEFAULT_SPAWNER: DefaultSpawner = ();
    }
}

impl DetachableTask<(), ()> {
    // ARCHITECTURE NOTE:
    // Anchoring constructors on `(), ()` instead of a bounded generic `impl` block
    // bypasses early trait bound resolution (e.g., `TaskSpawner<Task>`).
    // This prevents "type annotations needed" inference failures when passing opaque
    // `async { ... }` blocks whose types are not yet fully resolved.

    /// Creates a detachable task from an already-pinned, heap-allocated task.
    ///
    /// Unlike [`new`](Self::new), this accepts a pre-pinned
    /// `BoxTask<Task>` and does **not** perform an additional heap allocation.
    /// Use this when the task is already on the heap (e.g. produced by
    /// type-erased guards).
    pub fn from_boxed<Spawner: TaskSpawner<Task>, Task: ?Sized>(
        spawner: Spawner,
        task: BoxTask<Task>,
    ) -> DetachableTask<Spawner, Task> {
        DetachableTask {
            guard: ContextGuard::assemble(
                DetachableTaskContext {
                    spawner,
                    task: Some(task),
                },
                DetachableTaskGuard,
            ),
        }
    }

    /// Creates a detachable task with a custom spawner.
    ///
    /// The task starts in inline polling mode and only moves to `spawner`
    /// when detached (explicitly or by drop before completion).
    pub fn new<Spawner: TaskSpawner<Task>, Task>(
        spawner: Spawner,
        task: Task,
    ) -> DetachableTask<Spawner, Task> {
        Self::from_boxed(spawner, Box::pin(task))
    }
}

/// Creates a detachable task that uses the current Tokio runtime for
/// background detachment.
impl<Task> From<Task> for DetachableTask<DefaultSpawner, Task::IntoFuture>
where
    Task: IntoFuture,
    Task::IntoFuture: Send + 'static,
    <Task::IntoFuture as Future>::Output: Send + 'static,
{
    #[inline]
    fn from(value: Task) -> Self {
        DetachableTask::new(DEFAULT_SPAWNER, value.into_future())
    }
}

impl<Spawner: TaskSpawner<Task>, Task: ?Sized + Future> IntoFuture
    for DetachableTask<Spawner, Task>
{
    type Output = Task::Output;
    type IntoFuture = DetachableTaskFuture<Spawner, Task>;

    #[inline]
    fn into_future(self) -> Self::IntoFuture {
        DetachableTaskFuture { guard: self.guard }
    }
}

/// Future returned by [`DetachableTask::into_future`].
///
/// It polls the underlying task inline; if dropped while pending, drop logic on
/// the inner guard detaches the remainder to the configured spawner.
pub struct DetachableTaskFuture<Spawner: TaskSpawner<Task>, Task: ?Sized> {
    guard: DetachableTaskContextGuard<Spawner, Task>,
}

impl<Spawner: TaskSpawner<Task>, Task: ?Sized> Debug for DetachableTaskFuture<Spawner, Task> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DetachableTaskFuture")
            .finish_non_exhaustive()
    }
}

impl<Spawner: TaskSpawner<Task>, Task: ?Sized + Future> Future
    for DetachableTaskFuture<Spawner, Task>
{
    type Output = Task::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // SAFETY:
        // 1. We explicitly do not project the Pin to the `guard` field (structural unpinning).
        //    This is sound because the inner `Spawner` is eventually consumed by value
        //    and thus cannot rely on being pinned in memory.
        // 2. The inner task remains securely pinned on the heap via `BoxTask<Task>`.
        // 3. We never expose a mutable, unpinned reference to the underlying task.
        let this = unsafe { self.get_unchecked_mut() };
        let context = this.guard.deref_mut();
        // `take()` instead of `as_mut()` for panic safety: a panicking `poll()`
        // must not leave a potentially inconsistent future behind.
        let mut task = context.task.take().expect("task polled after completion");
        let poll = task.as_mut().poll(cx);
        if poll.is_pending() {
            context.task = Some(task);
        }
        poll
    }
}

impl<Spawner: TaskSpawner<Task>, Task: ?Sized + Future> FusedFuture
    for DetachableTaskFuture<Spawner, Task>
{
    #[inline]
    fn is_terminated(&self) -> bool {
        self.guard.task.is_none()
    }
}

// endregion

#[cfg(all(test, feature = "tokio"))]
mod tests {
    extern crate alloc;

    use super::*;
    use alloc::sync::Arc;
    use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use core::time::Duration;
    use tokio::sync::{mpsc, oneshot};

    #[tokio::test]
    async fn spawn_when_dropped() {
        let spawned = Arc::new(AtomicBool::new(false));
        {
            let spawned = spawned.clone();
            let _task = DetachableTask::from(async move {
                spawned.store(true, Ordering::SeqCst);
            });
        }

        tokio::time::timeout(Duration::from_secs(1), async {
            while !spawned.load(Ordering::SeqCst) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("task should be spawned on drop");
    }

    #[tokio::test]
    async fn await_completed_task_does_not_detach() {
        let spawn_count = Arc::new(AtomicUsize::new(0));
        let result = {
            let spawn_count = spawn_count.clone();
            DetachableTask::new(
                move |_| {
                    spawn_count.fetch_add(1, Ordering::SeqCst);
                },
                async { 7usize },
            )
            .await
        };

        assert_eq!(result, 7);
        assert_eq!(spawn_count.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn drop_without_await_and_runs_once() {
        let spawn_count = Arc::new(AtomicUsize::new(0));
        let (done_tx, done_rx) = oneshot::channel();

        {
            let spawn_count = spawn_count.clone();
            let _task = DetachableTask::new(
                move |f| {
                    spawn_count.fetch_add(1, Ordering::SeqCst);
                    tokio::spawn(async move {
                        let result = f.await;
                        let _ = done_tx.send(result);
                    });
                },
                async { 42usize },
            );
        }

        let detached_result = tokio::time::timeout(Duration::from_secs(1), done_rx)
            .await
            .expect("detached task should finish")
            .expect("detached task should send result");

        assert_eq!(detached_result, 42);
        assert_eq!(spawn_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn drop_after_await_still_detaches() {
        let spawn_count = Arc::new(AtomicUsize::new(0));
        let (value_tx, mut value_rx) = mpsc::channel(4);
        let (done_tx, done_rx) = oneshot::channel();

        let handle = {
            let future = async move {
                let mut sum = 0;
                while let Some(value) = value_rx.recv().await {
                    sum += value;
                }
                sum
            };

            let spawn_count = spawn_count.clone();
            let task = DetachableTask::new(
                move |f| {
                    spawn_count.fetch_add(1, Ordering::SeqCst);
                    tokio::spawn(async move {
                        let result = f.await;
                        let _ = done_tx.send(result);
                    });
                },
                future,
            );

            tokio::spawn(task.into_future())
        };

        value_tx
            .send(10)
            .await
            .expect("value receiver should still exist");
        handle.abort();
        value_tx
            .send(11)
            .await
            .expect("value receiver should still exist");
        drop(value_tx);

        let detached_result = tokio::time::timeout(Duration::from_secs(1), done_rx)
            .await
            .expect("detached polled task should finish")
            .expect("detached polled task should send result");

        assert_eq!(detached_result, 21);
        assert_eq!(spawn_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn panic_during_inline_poll_does_not_detach_on_drop() {
        struct PanicOnPollFuture {
            poll_count: Arc<AtomicUsize>,
        }

        impl Future for PanicOnPollFuture {
            type Output = ();

            fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Self::Output> {
                self.poll_count.fetch_add(1, Ordering::SeqCst);
                panic!("panic during inline poll")
            }
        }

        let poll_count = Arc::new(AtomicUsize::new(0));
        let detach_count = Arc::new(AtomicUsize::new(0));

        let task = {
            let detach_count = detach_count.clone();
            DetachableTask::new(
                move |_| {
                    detach_count.fetch_add(1, Ordering::SeqCst);
                },
                PanicOnPollFuture {
                    poll_count: poll_count.clone(),
                },
            )
        };

        let err = tokio::spawn(task.into_future())
            .await
            .expect_err("inline poll panic should propagate");

        assert!(err.is_panic());
        assert_eq!(poll_count.load(Ordering::SeqCst), 1);
        assert_eq!(detach_count.load(Ordering::SeqCst), 0);
    }
}
