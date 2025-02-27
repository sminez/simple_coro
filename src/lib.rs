//! An exploration of (ab)using async/await syntax to simplify writing state machines.
#![warn(
    clippy::complexity,
    clippy::correctness,
    clippy::style,
    future_incompatible,
    missing_debug_implementations,
    missing_docs,
    rust_2018_idioms,
    rustdoc::all,
    clippy::undocumented_unsafe_blocks
)]

use std::{
    cell::UnsafeCell,
    future::Future,
    marker::PhantomData,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll, Wake, Waker},
};

/// A strongly typed state machine that can send values back to a parent [Runner] each time it
/// yields, receiving a response in return.
pub trait StateMachine: Sized {
    /// The type that will be sent to the parent [Runner] at each await point
    type Snd: Unpin + 'static;
    /// The type that will be received from the parent [Runner] at each await point
    type Rcv: Unpin + 'static;
    /// The output of running this state machine to completion
    type Out;

    /// Return a future that can be executed by a [Runner] to run this state machine to completion.
    ///
    /// # Panics
    /// This [Future] must be executed using a [Runner] rather than awaiting it normally. Any calls
    /// to async methods or functions other than [Handle::yield_value] will panic.
    fn run(handle: Handle<Self::Snd, Self::Rcv>) -> impl Future<Output = Self::Out> + Send;

    /// Initialize a new [PinnedStateMachine] with the provided [RunState].
    fn initialize<R>(
        inner: R,
    ) -> PinnedStateMachine<R, Self::Out, impl Future<Output = Self::Out> + Send>
    where
        R: RunState<Snd = Self::Snd, Rcv = Self::Rcv>,
    {
        let state = Arc::new(State::default());
        let waker = Waker::from(state.clone());

        PinnedStateMachine {
            inner,
            state,
            waker,
            fut: Box::pin(Self::run(Handle {
                _snd: PhantomData,
                _rcv: PhantomData,
            })),
        }
    }
}

/// Additional unshared state for driving a [StateMachine] that is parameterised over the
/// specified `Snd` and `Rcv` types.
pub trait RunState: Sized {
    /// The type that will be sent from a [StateMachine] at each await point
    type Snd: Unpin + 'static;
    /// The type that will be returned to a [StateMachine] at each await point
    type Rcv: Unpin + 'static;
}

/// A handle to a running [StateMachine] that can make progress by calling [step][PinnedStateMachine::step].
#[allow(missing_debug_implementations)]
pub struct PinnedStateMachine<R, O, F>
where
    R: RunState,
    F: Future<Output = O> + Send,
{
    inner: R,
    state: Arc<State<R::Snd, R::Rcv>>,
    waker: Waker,
    fut: Pin<Box<F>>,
}

impl<R, O, F> PinnedStateMachine<R, O, F>
where
    R: RunState,
    F: Future<Output = O> + Send,
{
    /// Get a shared reference to the inner [RunState].
    pub fn inner(&self) -> &R {
        &self.inner
    }

    /// Get a exclusive reference to the inner [RunState].
    pub fn inner_mut(&mut self) -> &mut R {
        &mut self.inner
    }

    /// Run the [StateMachine] to its next yield point.
    pub fn step(&mut self) -> Step<R::Snd, O> {
        let mut ctx = Context::from_waker(&self.waker);
        let fut = &mut self.fut;

        match fut.as_mut().poll(&mut ctx) {
            Poll::Ready(val) => Step::Complete(val),
            Poll::Pending => Step::Pending(self.state.take_s()),
        }
    }

    /// Send a response to the child [StateMachine] following a call to [Handle::yield_value].
    pub fn send(&self, r: R::Rcv) {
        self.state.set_r(r);
    }
}

/// The intermediate state of a [StateMachine] while it is executing
#[derive(Debug)]
pub enum Step<S, T>
where
    S: Unpin,
{
    /// The [StateMachine] yielded via [Handle::yield_value]
    Pending(S),
    /// The [StateMachine] is now complete
    Complete(T),
}

#[derive(Debug)]
struct State<Snd, Rcv> {
    inner: UnsafeCell<StateInner<Snd, Rcv>>,
}

impl<S, R> Default for State<S, R> {
    fn default() -> Self {
        Self {
            inner: Default::default(),
        }
    }
}

/// SAFETY: We only ever access the shared state held within our [UnsafeCell] from a [StateMachine]
/// or its parent [Runner] which never execute at the same time.
unsafe impl<S, R> Send for State<S, R> {}
/// SAFETY: We only ever access the shared state held within our [UnsafeCell] from a [StateMachine]
/// or its parent [Runner] which never execute at the same time.
unsafe impl<S, R> Sync for State<S, R> {}

impl<S, R> State<S, R> {
    fn set_s(&self, s: S) {
        // SAFETY: We only ever access the shared state held within our UnsafeCell from a
        // StateMachine or its parent Runner which never execute at the same time.
        unsafe { (*self.inner.get()).s = Some(s) };
    }

    fn take_s(&self) -> S {
        // SAFETY: We only ever access the shared state held within our UnsafeCell from a
        // StateMachine or its parent Runner which never execute at the same time.
        unsafe {
            (*self.inner.get())
                .s
                .take()
                .expect("a StateMachine awaited a future other than Handle::yield_value")
        }
    }

    fn set_r(&self, r: R) {
        // SAFETY: We only ever access the shared state held within our UnsafeCell from a
        // StateMachine or its parent Runner which never execute at the same time.
        unsafe { (*self.inner.get()).r = Some(r) };
    }

    fn take_r(&self) -> R {
        // SAFETY: We only ever access the shared state held within our UnsafeCell from a
        // StateMachine or its parent Runner which never execute at the same time.
        unsafe {
            (*self.inner.get())
                .r
                .take()
                .expect("a Runner failed to set shared state before calling step")
        }
    }
}

#[derive(Debug)]
struct StateInner<S, R> {
    s: Option<S>,
    r: Option<R>,
}

impl<S, R> Default for StateInner<S, R> {
    fn default() -> Self {
        Self { s: None, r: None }
    }
}

impl<S, R> Wake for State<S, R> {
    fn wake(self: Arc<Self>) {}
    fn wake_by_ref(self: &Arc<Self>) {}
}

/// A yield handle to facilitate communication between a [StateMachine] and its parent [Runner].
///
/// The only way to obtain a [Handle] is via the [Runner::make_fut] method which will pass one to
/// [StateMachine::run] in order to construct the state machine future.
#[derive(Debug)]
pub struct Handle<S, R>
where
    S: Unpin + 'static,
    R: Unpin + 'static,
{
    _snd: PhantomData<S>,
    _rcv: PhantomData<R>,
}

impl<S, R> Clone for Handle<S, R>
where
    S: Unpin + 'static,
    R: Unpin + 'static,
{
    fn clone(&self) -> Self {
        *self
    }
}

impl<S, R> Copy for Handle<S, R>
where
    S: Unpin + 'static,
    R: Unpin + 'static,
{
}

impl<S, R> Handle<S, R>
where
    S: Unpin + 'static,
    R: Unpin + 'static,
{
    /// Yield back to the [Runner] that owns the [StateMachine] calling this method, requesting
    /// it to map an `S` into and `R`.
    pub async fn yield_value(&self, snd: S) -> R {
        Yield {
            polled: false,
            s: Some(snd),
            _r: PhantomData,
        }
        .await
    }
}

struct Yield<S, R>
where
    S: Unpin + 'static,
    R: Unpin + 'static,
{
    polled: bool,
    s: Option<S>,
    _r: PhantomData<R>,
}

impl<S, R> Future for Yield<S, R>
where
    S: Unpin + 'static,
    R: Unpin + 'static,
{
    type Output = R;

    fn poll(mut self: Pin<&mut Self>, ctx: &mut Context<'_>) -> Poll<R> {
        if self.polled {
            // SAFETY: we can only poll this future using a waker wrapping State<S, R>
            let data = unsafe {
                (ctx.waker().data() as *mut () as *mut State<S, R>)
                    .as_mut()
                    .unwrap_unchecked()
                    .take_r()
            };

            Poll::Ready(data)
        } else {
            self.polled = true;
            // SAFETY: we can only poll this future using a waker wrapping State<S, R>
            unsafe {
                (ctx.waker().data() as *mut () as *mut State<S, R>)
                    .as_mut()
                    .unwrap_unchecked()
                    .set_s(self.s.take().unwrap_unchecked());
            };

            Poll::Pending
        }
    }
}
