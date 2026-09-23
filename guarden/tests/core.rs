use guarden::task::DetachableTask;
use std::cell::Cell;
use std::future::{Future, IntoFuture, poll_fn};
use std::pin::Pin;
use std::task::{Context, Poll, Waker};

fn poll_once<F: Future + ?Sized>(future: Pin<&mut F>) -> Poll<F::Output> {
    future.poll(&mut Context::from_waker(Waker::noop()))
}

#[test]
fn custom_spawner_resumes_pending_work_without_a_runtime() {
    let polled = Cell::new(false);
    let future = async {
        poll_fn(|cx| {
            if polled.replace(true) {
                Poll::Ready(42)
            } else {
                cx.waker().wake_by_ref();
                Poll::Pending
            }
        })
        .await
    };
    let mut queued = None;
    // Also covers constructor inference with an opaque future and a borrowed,
    // non-Send spawner/future, without requiring the caller to name their types.
    let constructor = DetachableTask::new;
    let mut waiting = Box::pin(constructor(|task| queued = Some(task), future).into_future());
    assert_eq!(poll_once(waiting.as_mut()), Poll::Pending);
    drop(waiting);

    let mut task = queued.expect("spawner must retain the unfinished future");
    assert_eq!(poll_once(task.as_mut()), Poll::Ready(42));
}

#[cfg(not(feature = "tokio"))]
#[test]
fn default_async_apis_work_without_tokio() {
    use guarden::task::{DEFAULT_SPAWNER, TaskSpawner};
    use std::rc::Rc;

    let _: () = DEFAULT_SPAWNER;
    let mut task = ().spawn(Box::pin(async { 42 }));
    assert_eq!(poll_once(task.as_mut()), Poll::Ready(42));

    let mut task = Box::pin(DetachableTask::from(async { 42 }).into_future());
    assert_eq!(poll_once(task.as_mut()), Poll::Ready(42));
    let mut task = Box::pin(guarden::guard!(async { 42 }).trigger().into_future());
    assert_eq!(poll_once(task.as_mut()), Poll::Ready(42));

    let completed = Cell::new(false);
    let token = Rc::new(());
    {
        guarden::defer!([token = token.clone(), completed = &completed] async move {
            drop(token);
            completed.set(true);
        });
    }
    assert!(!completed.get());
    assert_eq!(Rc::strong_count(&token), 1);
}
