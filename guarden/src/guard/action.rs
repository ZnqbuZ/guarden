use crate::guard;
use crate::guard::Guard;
use crate::task::{DEFAULT_SPAWNER, DefaultSpawner, DetachableTask, TaskSpawner};
use core::fmt;
use core::fmt::Debug;
use core::future::Future;
use core::mem::take;
use std::pin::Pin;
use std::task::Context;
use std::task::Poll;

pub mod mode {
    mod private {
        use super::*;
        pub trait Sealed {}
        impl Sealed for Sync {}
        impl Sealed for Async {}
    }
    use private::*;

    pub trait Mode: Sealed {}

    /// A type-level token indicating synchronous execution mode.
    #[derive(Debug, Default, Clone, Copy)]
    pub struct Sync;

    /// A type-level token indicating asynchronous execution mode.
    #[derive(Debug, Default, Clone, Copy)]
    pub struct Async;

    impl Mode for Sync {}
    impl Mode for Async {}
}
use mode::*;

/// Defines the execution logic for a guard.
pub trait Action<Context> {
    /// The return type produced when the action is executed.
    type Output;
    /// Executes the action using the provided context.
    fn fire(self, context: Context) -> Self::Output;
}

// General sync: any closure FnOnce(Context) -> R
impl<Context, F, Output> Action<Context> for F
where
    F: FnOnce(Context) -> Output,
{
    type Output = Output;

    #[inline]
    fn fire(self, context: Context) -> Self::Output {
        self(context)
    }
}

#[derive(Debug, Default)]
pub(crate) enum ActionState<A, Output> {
    #[default]
    Empty,
    Armed(A),
    Fired(Output),
}

/// A wrapper that executes an asynchronous closure on a custom spawner when fired.
pub struct Spawn<Spawner, A, Task> {
    pub(crate) spawner: Option<Spawner>,
    pub(crate) state: ActionState<A, Task>,
}

impl<Spawner, A, Task> Debug for Spawn<Spawner, A, Task> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Spawn").finish_non_exhaustive()
    }
}

impl<Spawner, A, Task: Future> Future for Spawn<Spawner, A, Task> {
    type Output = Task::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = unsafe { self.get_unchecked_mut() };
        let state = &mut this.state as *mut ActionState<A, Task>;
        let ActionState::Fired(output) = (unsafe { &mut *state }) else {
            panic!("task polled after completion");
        };
        let guard = guard!([state] unsafe { *state = ActionState::Empty });
        let poll = unsafe { Pin::new_unchecked(output).poll(cx) };
        guard.defuse();
        poll
    }
}

impl<Spawner, A, Task> Spawn<Spawner, A, Task> {
    #[inline]
    pub(crate) fn ignite<Context>(&mut self, context: Context) -> Task
    where
        A: Action<Context, Output = Task>,
    {
        let ActionState::Armed(action) = take(&mut self.state) else {
            unreachable!("action evaluated multiple times");
        };
        action.fire(context)
    }
}

impl<Context, Spawner: TaskSpawner<Task>, A, Task> Action<Context> for Spawn<Spawner, A, Task>
where
    A: Action<Context, Output = Task>,
{
    type Output = DetachableTask<Spawner, Task>;

    #[inline]
    fn fire(mut self, context: Context) -> Self::Output {
        let spawner = self.spawner.take().expect("action fired multiple times");
        DetachableTask::new(spawner, self.ignite(context))
    }
}

/// Helper trait to infer the correct guard wrapper based on closure return type.
#[doc(hidden)]
pub trait Infer<M: Mode, Context> {
    type Action: Action<Context>;
    fn infer(self) -> Self::Action;
}

// Inferred sync: no wrapper, closure returns ()
impl<Context, F> Infer<Sync, Context> for F
where
    F: FnOnce(Context),
{
    type Action = F;

    #[inline]
    fn infer(self) -> Self::Action {
        self
    }
}

// Inferred async: wraps in Spawn with DefaultSpawner, closure returns Future
impl<Context, F, Task: Future> Infer<Async, Context> for F
where
    F: FnOnce(Context) -> Task,
    DefaultSpawner: TaskSpawner<Task>,
{
    type Action = Spawn<DefaultSpawner, F, Task>;

    #[inline]
    fn infer(self) -> Self::Action {
        Spawn {
            spawner: Some(DEFAULT_SPAWNER),
            state: ActionState::Armed(self),
        }
    }
}
