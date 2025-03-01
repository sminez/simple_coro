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
    fmt,
    future::Future,
    marker::PhantomData,
    pin::Pin,
    task::{Context, Poll, RawWaker, RawWakerVTable, Waker},
};

/// A future that can be executed as a [Coro].
/// See [AsCoro] and [IntoCoro] for details.
///
/// # Panics
/// Any calls to async methods or functions other than those provided by [Handle] will panic.
pub type GenFn<S, R, F> = fn(Handle<S, R>) -> F;

/// A type that can construct a [Coro]
pub trait AsCoro: Sized {
    /// The type that will be sent at each await point
    type Snd: Unpin + 'static;
    /// The type expected to be received at each await point
    type Rcv: Unpin + 'static;
    /// The output of running this state machine to completion
    type Out;

    /// Return a future that can be executed by calling [AsCoro::initialize] to convert this
    /// type into a [Coro] that can be run to completeion.
    ///
    /// # Panics
    /// Any calls to async methods or functions other than those provided by [Handle] will panic.
    fn as_coro(handle: Handle<Self::Snd, Self::Rcv>) -> impl Future<Output = Self::Out>;

    /// Initialize a new [Coro] using this type as a constructor.
    fn initialize() -> ReadyCoro<Self::Rcv, Self::Out, impl Future<Output = Self::Out>, Self::Snd> {
        Coro {
            _lifecyle: Ready,
            state: SharedState::default(),
            fut: Box::pin(Self::as_coro(Handle {
                _snd: PhantomData,
                _rcv: PhantomData,
            })),
        }
    }
}

/// A type that can be converted into a [Coro] from a value.
pub trait IntoCoro: Sized {
    /// The type that will be sent at each await point
    type Snd: Unpin + 'static;
    /// The type expected to be received at each await point
    type Rcv: Unpin + 'static;
    /// The output of running this state machine to completion
    type Out;

    /// Provide a [GenFn] to run as a [Coro].
    fn into_state_fn(self) -> GenFn<Self::Snd, Self::Rcv, impl Future<Output = Self::Out>>;

    /// Initialize a new [Coro] from a value.
    fn initialize(
        self,
    ) -> ReadyCoro<Self::Rcv, Self::Out, impl Future<Output = Self::Out>, Self::Snd> {
        Coro {
            _lifecyle: Ready,
            state: SharedState::default(),
            fut: Box::pin((self.into_state_fn())(Handle {
                _snd: PhantomData,
                _rcv: PhantomData,
            })),
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
        Coro {
            _lifecyle: Ready,
            state: SharedState::default(),
            fut: Box::pin((f)(Handle {
                _snd: PhantomData,
                _rcv: PhantomData,
            })),
        }
    }
}

#[derive(Debug)]
struct SharedState<S, R> {
    s: Option<S>,
    r: Option<R>,
}

impl<S, R> Default for SharedState<S, R> {
    fn default() -> Self {
        Self { s: None, r: None }
    }
}

/// The lifecycle state of a running [AsCoro]: either [Pending] or [Ready].
pub trait Lifecycle: fmt::Debug {}

/// Ready for the next call to [Coro::resume]
#[derive(Debug)]
pub struct Ready;
impl Lifecycle for Ready {}

/// Awaiting a call to [Coro::send] to reply to the inner [AsCoro].
#[derive(Debug)]
pub struct Pending;
impl Lifecycle for Pending {}

/// A [Coro] that is ready to be resumed
pub type ReadyCoro<R, O, F, S> = Coro<R, O, F, Ready, S>;

/// A [Coro] that is pending a response
pub type PendingCoro<R, O, F, S> = Coro<R, O, F, Pending, S>;

/// A handle to a running [AsCoro] that can make progress by calling [resume][Coro::resume].
pub struct Coro<R, O, F, L, S>
where
    R: Unpin + 'static,
    S: Unpin + 'static,
    F: Future<Output = O>,
    L: Lifecycle,
{
    _lifecyle: L,
    state: SharedState<S, R>,
    fut: Pin<Box<F>>,
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

const WAKER_VTABLE: RawWakerVTable = RawWakerVTable::new(clone_callback, |_| {}, |_| {}, |_| {});
unsafe fn clone_callback(ptr: *const ()) -> RawWaker {
    RawWaker::new(ptr, &WAKER_VTABLE)
}

impl<R, O, Fut, S> Coro<R, O, Fut, Ready, S>
where
    R: Unpin + 'static,
    S: Unpin + 'static,
    Fut: Future<Output = O> + Send,
{
    /// Run this [Coro] to completion using the provided synchronous step function.
    pub fn run_sync<F>(self, mut step_fn: F) -> O
    where
        F: FnMut(S) -> R,
    {
        let mut coro = self;
        loop {
            coro = {
                match coro.resume() {
                    CoroState::Complete(res) => return res,
                    CoroState::Pending(c, s) => c.send((step_fn)(s)),
                }
            };
        }
    }

    /// Run the [Coro] to its next yield point.
    pub fn resume(mut self) -> CoroState<S, O, R, Fut> {
        // SAFETY: we never use this waker for its intended purpose
        let waker = unsafe {
            Waker::from_raw(RawWaker::new(
                &self.state as *const SharedState<S, R> as *const (),
                &WAKER_VTABLE,
            ))
        };
        let mut ctx = Context::from_waker(&waker);
        match self.fut.as_mut().poll(&mut ctx) {
            Poll::Ready(val) => CoroState::Complete(val),
            Poll::Pending => {
                let s = self
                    .state
                    .s
                    .take()
                    .expect("a AsCoro awaited a future other than Handle::yield_value");

                let sm = Coro {
                    _lifecyle: Pending,
                    state: self.state,
                    fut: self.fut,
                };

                CoroState::Pending(sm, s)
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
    /// Send a response to the child [AsCoro] following a call to [Handle::yield_value].
    pub fn send(mut self, r: R) -> ReadyCoro<R, O, F, S> {
        self.state.r = Some(r);

        Coro {
            _lifecyle: Ready,
            state: self.state,
            fut: self.fut,
        }
    }
}

/// The intermediate state of a [AsCoro] while it is executing
#[allow(missing_debug_implementations)]
pub enum CoroState<S, T, R, F>
where
    S: Unpin + 'static,
    R: Unpin + 'static,
    F: Future<Output = T>,
{
    /// The [AsCoro] yielded via [Handle::yield_value]
    Pending(PendingCoro<R, T, F, S>, S),
    /// The [AsCoro] is now complete
    Complete(T),
}

/// A yield handle to facilitate communication between a [Coro] and the logic driving it.
///
/// The only way to obtain a [Handle] is via the [AsCoro::initialize] method which will pass one to
/// [AsCoro::as_coro] in order to construct the state machine future.
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
    /// Yield back to the code that owns the [Coro] calling this method, requesting it to
    /// map an `S` into and `R`.
    pub async fn yield_value(&self, snd: S) -> R {
        Yield {
            polled: false,
            s: Some(snd),
            _r: PhantomData,
        }
        .await
    }

    /// Defer yielding to another [Coro] until it completes
    pub async fn yield_from<T, C, F>(&self, coro: C) -> T
    where
        C: Into<ReadyCoro<R, T, F, S>>,
        F: Future<Output = T> + Send,
    {
        coro.into().fut.await
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
            // or inside of Coro::resume which never execute at the same time.
            let data = unsafe {
                (ctx.waker().data() as *mut () as *mut SharedState<S, R>)
                    .as_mut()
                    .unwrap_unchecked()
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
            // or inside of Coro::resume which never execute at the same time.
            unsafe {
                (ctx.waker().data() as *mut () as *mut SharedState<S, R>)
                    .as_mut()
                    .unwrap_unchecked()
                    .s = Some(self.s.take().unwrap_unchecked());
            };

            Poll::Pending
        }
    }
}
