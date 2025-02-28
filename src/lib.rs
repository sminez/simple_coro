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
    sync::Arc,
    task::{Context, Poll, Wake, Waker},
};

/// A future that can be executed by calling [IntoStateFn::initialize] to convert this
/// type into a [PinnedStateMachine] that can be run to completeion.
///
/// # Panics
/// Any calls to async methods or functions other than [Handle::yield_value] will panic.
pub type StateFn<S, R, F> = fn(Handle<S, R>) -> F;

impl<S, R, F, O> IntoStateFn for StateFn<S, R, F>
where
    S: Unpin + 'static,
    R: Unpin + 'static,
    F: Future<Output = O> + Send,
{
    type Snd = S;
    type Rcv = R;
    type Out = O;

    fn into_state_fn(
        self,
    ) -> StateFn<Self::Snd, Self::Rcv, impl Future<Output = Self::Out> + Send> {
        self
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
    fn into_state_fn(self)
    -> StateFn<Self::Snd, Self::Rcv, impl Future<Output = Self::Out> + Send>;

    /// Initialize a new [PinnedStateMachine] with the provided [RunState].
    fn initialize<R>(
        self,
        inner: R,
    ) -> PinnedStateMachine<R, Self::Out, impl Future<Output = Self::Out> + Send, Ready>
    where
        R: RunState<Snd = Self::Snd, Rcv = Self::Rcv>,
    {
        let state = Arc::new(State::default());
        let waker = Waker::from(state.clone());

        PinnedStateMachine {
            inner,
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

/// A strongly typed state machine that can send values back to a parent [Runner] each time it
/// yields, receiving a response in return.
pub trait StateMachine: Sized {
    /// The type that will be sent at each await point
    type Snd: Unpin + 'static;
    /// The type expected to be received at each await point
    type Rcv: Unpin + 'static;
    /// The output of running this state machine to completion
    type Out;

    /// Return a future that can be executed by calling [StateMachine::initialize] to convert this
    /// type into a [PinnedStateMachine] that can be run to completeion.
    ///
    /// # Panics
    /// Any calls to async methods or functions other than [Handle::yield_value] will panic.
    fn run(handle: Handle<Self::Snd, Self::Rcv>) -> impl Future<Output = Self::Out> + Send;

    /// Initialize a new [PinnedStateMachine] with the provided [RunState].
    fn initialize<R>(
        inner: R,
    ) -> PinnedStateMachine<R, Self::Out, impl Future<Output = Self::Out> + Send, Ready>
    where
        R: RunState<Snd = Self::Snd, Rcv = Self::Rcv>,
    {
        let state = Arc::new(State::default());
        let waker = Waker::from(state.clone());

        PinnedStateMachine {
            inner,
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

/// Additional unshared state for driving a [StateMachine] that is parameterised over the
/// specified `Snd` and `Rcv` types.
pub trait RunState: Sized {
    /// The type that will be sent from a [StateMachine] at each await point
    type Snd: Unpin + 'static;
    /// The type that will be returned to a [StateMachine] at each await point
    type Rcv: Unpin + 'static;
}

/// The lifecycle state of a running [StateMachine]: either [Pending] or [Ready].
pub trait Lifecycle: fmt::Debug {}

/// Ready for the next call to [PinnedStateMachine::step]
#[derive(Debug)]
pub struct Ready;
impl Lifecycle for Ready {}

/// Awaiting a call to [PinnedStateMachine::send] to reply to the inner [StateMachine].
#[derive(Debug)]
pub struct Pending;
impl Lifecycle for Pending {}

/// A handle to a running [StateMachine] that can make progress by calling [step][PinnedStateMachine::step].
pub struct PinnedStateMachine<R, O, F, L>
where
    R: RunState,
    F: Future<Output = O> + Send,
    L: Lifecycle,
{
    inner: R,
    _lifecyle: L,
    state: Arc<State<R::Snd, R::Rcv>>,
    waker: Waker,
    fut: Pin<Box<F>>,
}

impl<R, O, F, L> fmt::Debug for PinnedStateMachine<R, O, F, L>
where
    R: RunState,
    F: Future<Output = O> + Send,
    L: Lifecycle,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PinnedStateMachine")
            .field("lifecycle", &self._lifecyle)
            .finish()
    }
}

impl<R, O, F, L> PinnedStateMachine<R, O, F, L>
where
    R: RunState,
    F: Future<Output = O> + Send,
    L: Lifecycle,
{
    /// Get a shared reference to the inner [RunState].
    pub fn inner(&self) -> &R {
        &self.inner
    }

    /// Get a exclusive reference to the inner [RunState].
    pub fn inner_mut(&mut self) -> &mut R {
        &mut self.inner
    }
}

impl<R, O, F> PinnedStateMachine<R, O, F, Ready>
where
    R: RunState,
    F: Future<Output = O> + Send,
{
    /// Run the [StateMachine] to its next yield point.
    #[allow(clippy::type_complexity)]
    pub fn step(mut self) -> Step<R::Snd, O, R, F> {
        let mut ctx = Context::from_waker(&self.waker);
        match self.fut.as_mut().poll(&mut ctx) {
            Poll::Ready(val) => Step::Complete(val),
            Poll::Pending => {
                let s = self.state.take_s();
                let sm = PinnedStateMachine {
                    inner: self.inner,
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

impl<R, O, F> PinnedStateMachine<R, O, F, Pending>
where
    R: RunState,
    F: Future<Output = O> + Send,
{
    /// Send a response to the child [StateMachine] following a call to [Handle::yield_value].
    pub fn send(self, r: R::Rcv) -> PinnedStateMachine<R, O, F, Ready> {
        self.state.set_r(r);

        PinnedStateMachine {
            inner: self.inner,
            _lifecyle: Ready,
            state: self.state,
            waker: self.waker,
            fut: self.fut,
        }
    }
}

/// The intermediate state of a [StateMachine] while it is executing
#[derive(Debug)]
pub enum Step<S, T, R, F>
where
    S: Unpin,
    R: RunState,
    F: Future<Output = T> + Send,
{
    /// The [StateMachine] yielded via [Handle::yield_value]
    Pending(PinnedStateMachine<R, T, F, Pending>, S),
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
/// or inside of [PinnedStateMachine::step] which never execute at the same time.
unsafe impl<S, R> Send for State<S, R> {}
/// SAFETY: We only ever access the shared state held within our [UnsafeCell] from a [StateMachine]
/// or inside of [PinnedStateMachine::step] which never execute at the same time.
unsafe impl<S, R> Sync for State<S, R> {}

impl<S, R> State<S, R> {
    fn set_s(&self, s: S) {
        // SAFETY: We only ever access the shared state held within our UnsafeCell from a
        // or inside of PinnedStateMachine::step which never execute at the same time.
        unsafe { (*self.inner.get()).s = Some(s) };
    }

    fn take_s(&self) -> S {
        // SAFETY: We only ever access the shared state held within our UnsafeCell from a
        // or inside of PinnedStateMachine::step which never execute at the same time.
        unsafe {
            (*self.inner.get())
                .s
                .take()
                .expect("a StateMachine awaited a future other than Handle::yield_value")
        }
    }

    fn set_r(&self, r: R) {
        // SAFETY: We only ever access the shared state held within our UnsafeCell from a
        // or inside of PinnedStateMachine::step which never execute at the same time.
        unsafe { (*self.inner.get()).r = Some(r) };
    }

    fn take_r(&self) -> R {
        // SAFETY: We only ever access the shared state held within our UnsafeCell from a
        // or inside of PinnedStateMachine::step which never execute at the same time.
        unsafe {
            (*self.inner.get())
                .r
                .take()
                // should not be possible
                .expect("shared state was not set before calling step")
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

/// A yield handle to facilitate communication between a [PinnedStateMachine] and the logic driving it.
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
