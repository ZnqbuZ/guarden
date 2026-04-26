use crate::guard::ContextGuard;
use std::future::Future;
use std::ops::DerefMut;
use std::pin::Pin;
use std::task::{Context, Poll};

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
type DetachableTaskGuardHelper<Context> = ContextGuard<crate::guard::SyncMode, Context, fn(Context)>;
type DetachableTaskGuard<Spawner, Task> =
    DetachableTaskGuardHelper<DetachableTaskContext<Spawner, Task>>;

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
///    lost or require explicit propagation across task boundaries.
///
///    > **⚠️ WARNING on Detachment & Local State:**
///    > While local state is preserved *during inline execution*, if the task yields (e.g. `await`)
///    > and is subsequently detached (via Drop), the remaining execution will be transferred to a 
///    > newly spawned background task. At this point, **the caller's `task_local!` and TLS state 
///    > will be silently lost**. Do not rely on implicit local state across `.await` points inside
///    > the guarded future.
pub struct DetachableTask<Spawner, Task> {
    guard: DetachableTaskGuard<Spawner, Task>,
}

impl<Spawner, Task> DetachableTask<Spawner, Task> {
    pub fn detach(self) {
        self.guard.trigger()
    }

    pub fn reclaim(self) -> BoxTask<Task> {
        self.guard.defuse().task.unwrap()
    }
}

pub trait TaskSpawner<Task> {
    type Output;
    fn spawn(self, task: BoxTask<Task>) -> Self::Output
    where
        Self: Sized;
}

impl<F, Output, Task> TaskSpawner<Task> for F
where
    F: FnOnce(BoxTask<Task>) -> Output,
{
    type Output = Output;
    fn spawn(self, task: BoxTask<Task>) -> Self::Output {
        self(task)
    }
}

cfg_select! {
    feature = "tokio" => {
        use tokio::runtime::Handle;
        use tokio::task::JoinHandle;

        impl<Task> TaskSpawner<Task> for Handle
        where
            Task: Future + Send + 'static,
            <Task as Future>::Output: Send + 'static,
        {
            type Output = JoinHandle<<Task as Future>::Output>;
            fn spawn(self, task: BoxTask<Task>) -> Self::Output {
                Handle::spawn(&self, task)
            }
        }
    }
}

impl DetachableTask<fn(()), ()> {
    pub fn with_spawner<Spawner, Task>(
        spawner: Spawner,
        task: Task,
    ) -> DetachableTask<Spawner, Task>
    where
        Spawner: TaskSpawner<Task>,
    {
        let context = DetachableTaskContext {
            spawner,
            task: Some(Box::pin(task)),
        };
        DetachableTask {
            guard: crate::guard!([context] if let Some(task) = context.task {
                context.spawner.spawn(task);
            }),
        }
    }

    #[cfg(feature = "tokio")]
    pub fn new<Task>(task: Task) -> DetachableTask<Handle, Task>
    where
        Task: Future + Send + 'static,
        <Task as Future>::Output: Send + 'static,
    {
        let handle = Handle::current();
        Self::with_spawner(handle, task)
    }
}

impl<Spawner: TaskSpawner<Task>, Task> IntoFuture for DetachableTask<Spawner, Task>
where
    Task: Future,
{
    type Output = Task::Output;
    type IntoFuture = DetachableTaskFuture<Spawner, Task>;

    fn into_future(self) -> Self::IntoFuture {
        DetachableTaskFuture { guard: self.guard }
    }
}

pub struct DetachableTaskFuture<Spawner, Task> {
    guard: DetachableTaskGuard<Spawner, Task>,
}

impl<Spawner: TaskSpawner<Task>, Task> Future for DetachableTaskFuture<Spawner, Task>
where
    Task: Future,
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
        let mut task = context.task.take().expect("polled after completion");
        let poll = task.as_mut().poll(cx);
        if poll.is_pending() {
            context.task = Some(task);
        }
        poll
    }
}

// endregion

#[cfg(all(test, feature = "tokio"))]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;
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
