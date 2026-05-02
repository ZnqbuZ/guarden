use crate::guard::{CallableGuard, ContextGuard};
use alloc::boxed::Box;
use core::future::Future;
use core::ops::DerefMut;
use core::pin::Pin;
use core::task::{Context, Poll};
use futures_core::future::FusedFuture;

// region DetachableTask

/// A pinned, heap-allocated task.
///
/// **Why Box?** Heap allocation is required because if the task detaches,
/// it outlives the current stack frame. `Pin<Box<_>>` ensures its memory address
/// remains completely stable during and after the transfer.
type BoxTask<Task> = Pin<Box<Task>>;

struct DetachableTaskContext<Spawner, Task> {
    spawner: Spawner,
    task: Option<BoxTask<Task>>,
}

struct DetachableTaskGuard;

impl<Spawner: TaskSpawner<Task>, Task>
    CallableGuard<false, false, DetachableTaskContext<Spawner, Task>> for DetachableTaskGuard
{
    type Output = ();

    #[inline]
    fn call(self, context: DetachableTaskContext<Spawner, Task>) {
        if let Some(task) = context.task {
            context.spawner.spawn(task);
        }
    }
}

type DetachableTaskContextGuard<Spawner, Task> =
    ContextGuard<false, false, DetachableTaskContext<Spawner, Task>, DetachableTaskGuard>;

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
pub struct DetachableTask<Spawner: TaskSpawner<Task>, Task> {
    guard: DetachableTaskContextGuard<Spawner, Task>,
}

impl<Spawner: TaskSpawner<Task>, Task> DetachableTask<Spawner, Task> {
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
pub trait TaskSpawner<Task> {
    /// Return type of the spawn operation.
    type Output;

    /// Consumes `self` and schedules `task` for background execution.
    fn spawn(self, task: BoxTask<Task>) -> Self::Output
    where
        Self: Sized;
}

impl<F, Output, Task> TaskSpawner<Task> for F
where
    F: FnOnce(BoxTask<Task>) -> Output,
{
    type Output = Output;

    #[inline]
    fn spawn(self, task: BoxTask<Task>) -> Self::Output {
        self(task)
    }
}

impl<Task> TaskSpawner<Task> for () {
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
        pub struct TokioHandle;

        impl<Task> TaskSpawner<Task> for TokioHandle
        where
            Task: Future + Send + 'static,
            <Task as Future>::Output: Send + 'static,
        {
            type Output = JoinHandle<<Task as Future>::Output>;
            fn spawn(self, task: BoxTask<Task>) -> Self::Output {
                Handle::current().spawn(task)
            }
        }
    }

    _ => {}
}

impl DetachableTask<(), ()> {
    /// Creates a detachable task with a custom spawner.
    ///
    /// The task starts in inline polling mode and only moves to `spawner`
    /// when detached (explicitly or by drop before completion).
    pub fn with_spawner<Spawner: TaskSpawner<Task>, Task>(
        spawner: Spawner,
        task: Task,
    ) -> DetachableTask<Spawner, Task> {
        let context = DetachableTaskContext {
            spawner,
            task: Some(Box::pin(task)),
        };
        DetachableTask {
            guard: ContextGuard::with_guard(context, DetachableTaskGuard),
        }
    }

    /// Creates a detachable task that uses the current Tokio runtime for
    /// background detachment.
    #[cfg(feature = "tokio")]
    #[inline]
    pub fn new<Task>(task: Task) -> DetachableTask<TokioHandle, Task>
    where
        Task: Future + Send + 'static,
        <Task as Future>::Output: Send + 'static,
    {
        Self::with_spawner(TokioHandle, task)
    }
}

impl<Spawner: TaskSpawner<Task>, Task: Future> IntoFuture for DetachableTask<Spawner, Task> {
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
pub struct DetachableTaskFuture<Spawner: TaskSpawner<Task>, Task> {
    guard: DetachableTaskContextGuard<Spawner, Task>,
}

impl<Spawner: TaskSpawner<Task>, Task: Future> Future for DetachableTaskFuture<Spawner, Task> {
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
        let mut task = context.task.take().expect("polled after completion");
        let poll = task.as_mut().poll(cx);
        if poll.is_pending() {
            context.task = Some(task);
        }
        poll
    }
}

impl<Spawner: TaskSpawner<Task>, Task: Future> FusedFuture for DetachableTaskFuture<Spawner, Task> {
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
            let _task = DetachableTask::new(async move {
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
            DetachableTask::with_spawner(
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
            let _task = DetachableTask::with_spawner(
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
            let task = DetachableTask::with_spawner(
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
            DetachableTask::with_spawner(
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
