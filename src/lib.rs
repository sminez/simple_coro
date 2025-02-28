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
    fmt,
    future::Future,
    marker::PhantomData,
    pin::Pin,
    task::{Context, Poll, RawWaker, RawWakerVTable, Waker},
};

/// A future that can be executed by calling [StateMachine::initialize] to convert this
/// type into a [Coro] that can be run to completeion.
///
/// # Panics
/// Any calls to async methods or functions other than [Handle::yield_value] will panic.
pub type StateFn<S, R, F> = fn(Handle<S, R>) -> F;

/// A strongly typed state machine that can send values back to a caller each time it yields,
/// receiving a response in return.
pub trait StateMachine: Sized {
    /// The type that will be sent at each await point
    type Snd: Unpin + 'static;
    /// The type expected to be received at each await point
    type Rcv: Unpin + 'static;
    /// The output of running this state machine to completion
    type Out;

    /// Return a future that can be executed by calling [StateMachine::initialize] to convert this
    /// type into a [Coro] that can be run to completeion.
    ///
    /// # Panics
    /// Any calls to async methods or functions other than [Handle::yield_value] will panic.
    fn run(handle: Handle<Self::Snd, Self::Rcv>) -> impl Future<Output = Self::Out>;

    /// Initialize a new [Coro].
    fn initialize() -> ReadyCoro<Self::Rcv, Self::Out, impl Future<Output = Self::Out>, Self::Snd> {
        let (state, waker) = init_state();

        Coro {
            _lifecyle: Ready,
            state,
            waker,
            fut: Box::pin(Self::run(Handle {
                _snd: PhantomData,
                _rcv: PhantomData,
            })),
        }
    }
}

/// The lifecycle state of a running [StateMachine]: either [Pending] or [Ready].
pub trait Lifecycle: fmt::Debug {}

/// Ready for the next call to [Coro::step]
#[derive(Debug)]
pub struct Ready;
impl Lifecycle for Ready {}

/// Awaiting a call to [Coro::send] to reply to the inner [StateMachine].
#[derive(Debug)]
pub struct Pending;
impl Lifecycle for Pending {}

/// A [Coro] that is ready to be stepped
pub type ReadyCoro<R, O, F, S> = Coro<R, O, F, Ready, S>;

/// A [Coro] that is pending a response
pub type PendingCoro<R, O, F, S> = Coro<R, O, F, Pending, S>;

/// A handle to a running [StateMachine] that can make progress by calling [step][Coro::step].
pub struct Coro<R, O, F, L, S>
where
    R: Unpin + 'static,
    S: Unpin + 'static,
    F: Future<Output = O>,
    L: Lifecycle,
{
    _lifecyle: L,
    state: StatePtr<S, R>,
    waker: Waker,
    fut: Pin<Box<F>>,
}

struct StatePtr<S, R>(*const State<S, R>)
where
    R: Unpin + 'static,
    S: Unpin + 'static;

/// SAFETY: a StatePtr is only ever wrapping a pointer on the heap that is shared between a
/// Coro and the StateFn it holds
unsafe impl<S, R> Send for StatePtr<S, R>
where
    R: Send + Unpin + 'static,
    S: Send + Unpin + 'static,
{
}

/// SAFETY: a StatePtr is only ever wrapping a pointer on the heap that is shared between a
/// Coro and the StateFn it holds
unsafe impl<S, R> Sync for StatePtr<S, R>
where
    R: Sync + Unpin + 'static,
    S: Sync + Unpin + 'static,
{
}

impl<S, R> Drop for StatePtr<S, R>
where
    R: Unpin + 'static,
    S: Unpin + 'static,
{
    fn drop(&mut self) {
        // SAFETY: the only other copies of this pointer are in the StateFn that is being
        // dropped at the same time and it is not currently running if we are dropping the
        // Coro containing it.
        unsafe { std::ptr::drop_in_place(self.0 as *mut State<S, R>) };
    }
}

impl<R, O, F, L, S> fmt::Debug for Coro<R, O, F, L, S>
where
    R: Unpin + 'static,
    S: Unpin + 'static,
    F: Future<Output = O> + Send,
    L: Lifecycle,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Coro")
            .field("lifecycle", &self._lifecyle)
            .finish()
    }
}

impl<R, O, F, S> Coro<R, O, F, Ready, S>
where
    R: Unpin + 'static,
    S: Unpin + 'static,
    F: Future<Output = O> + Send,
{
    /// Run the [StateMachine] to its next yield point.
    #[allow(clippy::type_complexity)]
    pub fn step(mut self) -> Step<S, O, R, F> {
        let mut ctx = Context::from_waker(&self.waker);
        match self.fut.as_mut().poll(&mut ctx) {
            Poll::Ready(val) => Step::Complete(val),
            Poll::Pending => {
                // SAFETY: We only ever access the shared state held within our UnsafeCell from a
                // or inside of Coro::step which never execute at the same time.
                let s = unsafe {
                    (*(*self.state.0).inner.get())
                        .s
                        .take()
                        .expect("a StateMachine awaited a future other than Handle::yield_value")
                };

                let sm = Coro {
                    _lifecyle: Pending,
                    state: self.state,
                    waker: self.waker,
                    fut: self.fut,
                };

                Step::Pending(sm, s)
            }
        }
    }
}

impl<R, O, F, S> Coro<R, O, F, Pending, S>
where
    R: Unpin + 'static,
    S: Unpin + 'static,
    F: Future<Output = O>,
{
    /// Send a response to the child [StateMachine] following a call to [Handle::yield_value].
    pub fn send(self, r: R) -> ReadyCoro<R, O, F, S> {
        // SAFETY: We only ever access the shared state held within our UnsafeCell from a
        // or inside of Coro::step which never execute at the same time.
        unsafe { (*(*self.state.0).inner.get()).r = Some(r) };

        Coro {
            _lifecyle: Ready,
            state: self.state,
            waker: self.waker,
            fut: self.fut,
        }
    }
}

/// The intermediate state of a [StateMachine] while it is executing
#[allow(missing_debug_implementations)]
pub enum Step<S, T, R, F>
where
    S: Unpin + 'static,
    R: Unpin + 'static,
    F: Future<Output = T>,
{
    /// The [StateMachine] yielded via [Handle::yield_value]
    Pending(PendingCoro<R, T, F, S>, S),
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

const WAKER_VTABLE: RawWakerVTable = RawWakerVTable::new(clone_callback, |_| {}, |_| {}, |_| {});
unsafe fn clone_callback(ptr: *const ()) -> RawWaker {
    RawWaker::new(ptr, &WAKER_VTABLE)
}

fn init_state<S, R>() -> (StatePtr<S, R>, Waker)
where
    R: Unpin + 'static,
    S: Unpin + 'static,
{
    let state = Box::into_raw(Box::new(State::default()));
    // SAFETY: we never use this waker for its intended purpose
    let waker = unsafe { Waker::from_raw(RawWaker::new(state as *mut (), &WAKER_VTABLE)) };

    (StatePtr(state), waker)
}

/// A yield handle to facilitate communication between a [Coro] and the logic driving it.
///
/// The only way to obtain a [Handle] is via the [StateMachine::initialize] method which will pass one to
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
    /// Yield back to the code that owns the [StateMachine] calling this method, requesting it to
    /// map an `S` into and `R`.
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
            // and we only ever access the shared state held within our UnsafeCell from a
            // or inside of Coro::step which never execute at the same time.
            let data = unsafe {
                (*(ctx.waker().data() as *mut () as *mut State<S, R>)
                    .as_mut()
                    .unwrap_unchecked()
                    .inner
                    .get())
                .r
                .take()
                // should not be possible
                .expect("shared state was not set before calling step")
            };

            Poll::Ready(data)
        } else {
            self.polled = true;
            // SAFETY: we can only poll this future using a waker wrapping State<S, R>
            // and we only ever access the shared state held within our UnsafeCell from a
            // or inside of Coro::step which never execute at the same time.
            unsafe {
                (*(ctx.waker().data() as *mut () as *mut State<S, R>)
                    .as_mut()
                    .unwrap_unchecked()
                    .inner
                    .get())
                .s = Some(self.s.take().unwrap_unchecked());
            };

            Poll::Pending
        }
    }
}

impl<F, S, R, Fut, O> From<F> for ReadyCoro<R, O, Fut, S>
where
    F: Fn(Handle<S, R>) -> Fut,
    S: Unpin + 'static,
    R: Unpin + 'static,
    Fut: Future<Output = O>,
{
    fn from(f: F) -> Self {
        let (state, waker) = init_state();

        Coro {
            _lifecyle: Ready,
            state,
            waker,
            fut: Box::pin((f)(Handle {
                _snd: PhantomData,
                _rcv: PhantomData,
            })),
        }
    }
}

/// A type that can be converted into a [StateFn]
pub trait IntoStateFn: Sized {
    /// The type that will be sent at each await point
    type Snd: Unpin + 'static;
    /// The type expected to be received at each await point
    type Rcv: Unpin + 'static;
    /// The output of running this state machine to completion
    type Out;

    /// Provide a [StateFn] to run as the state machine
    fn into_state_fn(self) -> StateFn<Self::Snd, Self::Rcv, impl Future<Output = Self::Out>>;

    /// Initialize a new [Coro].
    fn initialize(
        self,
    ) -> ReadyCoro<Self::Rcv, Self::Out, impl Future<Output = Self::Out>, Self::Snd> {
        let (state, waker) = init_state();

        Coro {
            _lifecyle: Ready,
            state,
            waker,
            fut: Box::pin((self.into_state_fn())(Handle {
                _snd: PhantomData,
                _rcv: PhantomData,
            })),
        }
    }
}
