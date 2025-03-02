#![doc = include_str!("../docs.md")]
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
/// Calls to async methods or functions that attempt to make use of a [Waker] will panic.
pub type CoroFn<S, R, F> = fn(Handle<S, R>) -> F;

/// A type that can construct a [Coro].
///
/// If you need to make use of data held within `self` in order to produce your [CoroFn] then you
/// should look at implementing [IntoCoro] instead.
///
/// # Example
/// ```rust
/// # use crimes::{AsCoro, Handle};
/// // Implementing a simple counter using a coroutine (equivalent to std::iter::from_fn)
/// struct Counter<const N: usize>;
///
/// impl<const N: usize> AsCoro for Counter<N> {
///     type Snd = usize;
///     type Rcv = ();
///     type Out = ();
///
///     async fn as_coro_fn(handle: Handle<usize>) {
///         for n in 0..N {
///             handle.yield_value(n).await
///         }
///     }
/// }
///
///
/// let nums: Vec<usize> = Counter::<6>::as_coro().collect();
/// assert_eq!(nums, vec![0, 1, 2, 3, 4, 5]);
/// ```
pub trait AsCoro {
    /// The type that will be sent at each await point
    type Snd: Unpin + 'static;
    /// The type expected to be received at each await point
    type Rcv: Unpin + 'static;
    /// The output of running the coroutine to completion
    type Out;

    /// Return a future that can be executed by calling [AsCoro::as_coro] to convert this type into
    /// a [Coro] that can be run to completeion.
    ///
    /// # Panics
    /// Calls to async methods or functions that attempt to make use of a [Waker] will panic.
    fn as_coro_fn(handle: Handle<Self::Snd, Self::Rcv>) -> impl Future<Output = Self::Out>;

    /// Initialize a new [Coro] using this type as a constructor.
    fn as_coro() -> ReadyCoro<Self::Rcv, Self::Out, impl Future<Output = Self::Out>, Self::Snd> {
        Coro {
            _lifecycle: Ready,
            state: SharedState::default(),
            fut: Box::pin(Self::as_coro_fn(Handle {
                _snd: PhantomData,
                _rcv: PhantomData,
            })),
        }
    }
}

/// A type that can be converted into a [Coro] from a value.
///
/// If you don't need to make use of data held within `self` in order to produce your [CoroFn] then
/// you should look at implementing [AsCoro] instead.
///
/// ```rust
/// # use crimes::{IntoCoro, Handle};
/// struct Echo<T> {
///     initial: T,
/// }
///
/// impl<T: Unpin + 'static> IntoCoro for Echo<T> {
///     type Snd = T;
///     type Rcv = T;
///     type Out = ();
///
///     async fn into_coro_fn(self, handle: Handle<T, T>) {
///         let mut val = self.initial;
///         loop {
///             val = handle.yield_value(val).await;
///         }
///     }
/// }
/// ```
pub trait IntoCoro: Sized {
    /// The type that will be sent at each await point
    type Snd: Unpin + 'static;
    /// The type expected to be received at each await point
    type Rcv: Unpin + 'static;
    /// The output of running the coroutine to completion
    type Out;

    /// Provide a future that can be executed by calling [IntoCoro::into_coro] to convert this type
    /// into a [Coro] that can be run to completeion.
    ///
    /// # Panics
    /// Calls to async methods or functions that attempt to make use of a [Waker] will panic.
    fn into_coro_fn(self, handle: Handle<Self::Snd, Self::Rcv>) -> impl Future<Output = Self::Out>;

    /// Initialize a new [Coro] from a value.
    fn into_coro(
        self,
    ) -> ReadyCoro<Self::Rcv, Self::Out, impl Future<Output = Self::Out>, Self::Snd> {
        Coro {
            _lifecycle: Ready,
            state: SharedState::default(),
            fut: Box::pin(self.into_coro_fn(Handle {
                _snd: PhantomData,
                _rcv: PhantomData,
            })),
        }
    }
}

// Closures with the correct signature can be directly converted into a [Coro].
impl<F, S, R, Fut, O> From<F> for ReadyCoro<R, O, Fut, S>
where
    F: FnMut(Handle<S, R>) -> Fut,
    S: Unpin + 'static,
    R: Unpin + 'static,
    Fut: Future<Output = O>,
{
    fn from(mut f: F) -> Self {
        Coro {
            _lifecycle: Ready,
            state: SharedState::default(),
            fut: Box::pin((f)(Handle {
                _snd: PhantomData,
                _rcv: PhantomData,
            })),
        }
    }
}

const DENY_FUT: &str = "a Coro is not permitted to await a future that uses an arbitrary Waker";
const WAKER_VTABLE: RawWakerVTable = RawWakerVTable::new(
    |_| panic!("{DENY_FUT}"),
    |_| panic!("{DENY_FUT}"),
    |_| panic!("{DENY_FUT}"),
    |_| {},
);

#[derive(Debug, Clone)]
struct SharedState<S, R> {
    s: Option<S>,
    r: Option<R>,
}

impl<S, R> Default for SharedState<S, R> {
    fn default() -> Self {
        Self { s: None, r: None }
    }
}

/// The lifecycle state of a running [Coro]: either [Pending] or [Ready].
pub trait Lifecycle: fmt::Debug {}

/// Ready for the next call to [Coro::resume]
#[derive(Debug)]
pub struct Ready;
impl Lifecycle for Ready {}

/// Awaiting a call to [Coro::send].
#[derive(Debug)]
pub struct Pending;
impl Lifecycle for Pending {}

/// A [Coro] that is ready to be resumed by calling [resume][Coro::resume].
pub type ReadyCoro<R, O, F, S> = Coro<R, O, F, Ready, S>;

/// A [Coro] that is pending a response from [send][Coro::send].
pub type PendingCoro<R, O, F, S> = Coro<R, O, F, Pending, S>;

/// A running coroutine that can make progress by calling [resume][Coro::resume].
///
/// A `Coro` can be constructed directly from a [CoroFn] using `Coro::from` or via either of the
/// [AsCoro] or [IntoCoro] traits.
pub struct Coro<R, O, F, L, S>
where
    R: Unpin + 'static,
    S: Unpin + 'static,
    F: Future<Output = O>,
    L: Lifecycle,
{
    _lifecycle: L,
    state: SharedState<S, R>,
    fut: Pin<Box<F>>,
}

impl<R, O, F, L, S> fmt::Debug for Coro<R, O, F, L, S>
where
    R: Unpin + 'static,
    S: Unpin + 'static,
    F: Future<Output = O>,
    L: Lifecycle,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Coro")
            .field("lifecycle", &self._lifecycle)
            .finish()
    }
}

impl<R, O, Fut, S> Coro<R, O, Fut, Ready, S>
where
    R: Unpin + 'static,
    S: Unpin + 'static,
    Fut: Future<Output = O>,
{
    /// Run this [Coro] to completion using the provided synchronous step function. If you need to
    /// make use of an asynchronous step function you will need to write out the equivalent loop by
    /// hand.
    ///
    /// # Example
    /// ```rust
    /// # use crimes::{Coro, Handle};
    /// let my_coro = Coro::from(async |handle: Handle<usize, Option<usize>>| {
    ///     let pos = handle.yield_value(42).await;
    ///     assert_eq!(pos, Some(5));
    ///     let pos = handle.yield_value(17).await;
    ///     assert_eq!(pos, Some(2));
    ///     let pos = handle.yield_value(68).await;
    ///     assert!(pos.is_none());
    ///
    ///     "done"
    /// });
    ///
    /// let vec = vec![5, 3, 17, 29, 103, 42, 55];
    /// let res = my_coro.run_sync(|n| vec.iter().position(|&x| x == n));
    /// assert_eq!(res, "done");
    /// ```
    ///
    /// Equivalent to:
    /// ```rust
    /// # use crimes::{Coro, CoroState, HandOwl};
    /// # let my_coro = Coro::from(async |_: HandOwl| {});
    /// # let step_fn = |unit| unit;
    /// let mut coro = my_coro;
    /// loop {
    ///     coro = match coro.resume() {
    ///         CoroState::Complete(res) => return res,
    ///         CoroState::Pending(c, s) => c.send((step_fn)(s)),
    ///     };
    /// }
    /// ```
    pub fn run_sync<F>(self, mut step_fn: F) -> O
    where
        F: FnMut(S) -> R,
    {
        let mut coro = self;
        loop {
            coro = match coro.resume() {
                CoroState::Complete(res) => return res,
                CoroState::Pending(c, s) => c.send((step_fn)(s)),
            };
        }
    }

    /// Run the [Coro] to its next yield point, returning a [CoroState].
    ///
    /// ```rust
    /// # use crimes::{Coro, CoroState, HandOwl};
    /// # let coro = Coro::from(async |_: HandOwl| {});
    /// match coro.resume() {
    ///     CoroState::Complete(res) => println!("coroutine completed with value: {res:?}"),
    ///     CoroState::Pending(pending_coro, val) => println!("coroutine yielded {val:?}"),
    /// };
    /// ```
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
                let s = self.state.s.take().unwrap_or_else(|| panic!("{DENY_FUT}"));
                let sm = Coro {
                    _lifecycle: Pending,
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
    /// Send a response to this [Coro] following a call to [Handle::yield_value].
    ///
    /// Calling this method coverts a [PendingCoro] into a [ReadyCoro], allowing it to be resumed
    /// again (and reassigned to an existing variable).
    ///
    /// ```rust
    /// # use crimes::{Coro, CoroState, Handle};
    /// let mut coro = Coro::from(async |handle: Handle<(), usize>| {
    ///     let n = handle.yield_value(()).await;
    ///     assert_eq!(n, 70);
    /// });
    ///
    /// coro = match coro.resume() {
    ///     CoroState::Pending(pending_coro, _) => pending_coro.send(70),
    ///     CoroState::Complete(_) => return,
    /// };
    /// ```
    pub fn send(mut self, r: R) -> ReadyCoro<R, O, F, S> {
        self.state.r = Some(r);

        Coro {
            _lifecycle: Ready,
            state: self.state,
            fut: self.fut,
        }
    }
}

/// A Generator is a simplified [Coro] that only emits values and does not return a final value.
/// As such, it can be used as a convenient way to write an [Iterator].
pub type Generator<T, F> = Coro<(), (), F, Ready, T>;

impl<T, F> Iterator for Generator<T, F>
where
    T: Unpin + 'static,
    F: Future<Output = ()>,
{
    type Item = T;

    /// The impl here is a modified version of the [Coro::resume] and [Coro::send] methods that
    /// avoids flipping the typestate between [Ready] and [Pending].
    fn next(&mut self) -> Option<T> {
        // SAFETY: we never use this waker for its intended purpose
        let waker = unsafe {
            Waker::from_raw(RawWaker::new(
                &self.state as *const SharedState<T, ()> as *const (),
                &WAKER_VTABLE,
            ))
        };
        let mut ctx = Context::from_waker(&waker);
        match self.fut.as_mut().poll(&mut ctx) {
            Poll::Pending => {
                let val = self.state.s.take().unwrap_or_else(|| panic!("{DENY_FUT}"));
                self.state.r = Some(());

                Some(val)
            }
            Poll::Ready(()) => None,
        }
    }
}

/// The intermediate state of a [Coro] while it is executing
#[derive(Debug)]
#[must_use]
pub enum CoroState<S, T, R, F>
where
    S: Unpin + 'static,
    R: Unpin + 'static,
    F: Future<Output = T>,
{
    /// The [Coro] yielded via [yield_value][Handle::yield_value]. Call the [send][Coro::send]
    /// method on the inner [PendingCoro] to send a value back into the coroutine and convert
    /// it back to a [ReadyCoro] which can be [resumed][Coro::resume].
    Pending(PendingCoro<R, T, F, S>, S),
    /// The [Coro] is now complete and has returned a value.
    Complete(T),
}

impl<S, T, R, F> CoroState<S, T, R, F>
where
    S: Unpin + 'static,
    R: Unpin + 'static,
    F: Future<Output = T>,
{
    /// Returns `true` if this is a [CoroState::Pending] value.
    pub fn is_pending(&self) -> bool {
        matches!(self, &Self::Pending(_, _))
    }

    /// Returns `true` if this is a [CoroState::Complete] value.
    pub fn is_complete(&self) -> bool {
        matches!(self, &Self::Complete(_))
    }

    /// Assume that this value is [Pending][CoroState::Pending] and send a response to the
    /// underlying [Coro] using the provided mapping function to unwrap it into a [ReadyCoro].
    ///
    /// # Panics
    /// This will panic if the value was [Complete][CoroState::Complete].
    pub fn unwrap_pending(self, f: impl Fn(S) -> R) -> ReadyCoro<R, T, F, S> {
        match self {
            Self::Pending(coro, s) => coro.send((f)(s)),
            Self::Complete(_) => {
                panic!("called `CoroState::unwrap_pending` on a `Complete` value")
            }
        }
    }

    /// Assume that this value is [Complete][CoroState::Complete] and unwrap it to return the inner
    /// value.
    ///
    /// # Panics
    /// This will panic if the value was [Pending][CoroState::Pending].
    pub fn unwrap(self) -> T {
        match self {
            Self::Pending(_, _) => {
                panic!("called `CoroState::unwrap` on a `Pending` value")
            }
            Self::Complete(t) => t,
        }
    }
}

/// A yield handle to facilitate communication between a [Coro] and the logic driving it.
///
/// The only way to obtain a [Handle] is by constructing a [Coro] (via the [AsCoro::as_coro_fn] or
/// [IntoCoro::into_coro_fn] methods or calling [Coro::from] on an appropriate function).
#[derive(Debug)]
pub struct Handle<S, R = ()>
where
    S: Unpin + 'static,
    R: Unpin + 'static,
{
    _snd: PhantomData<S>,
    _rcv: PhantomData<R>,
}

#[doc(hidden)]
pub type HandOwl = Handle<(), ()>;

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
        F: Future<Output = T>,
    {
        coro.into().fut.await
    }

    /// Defer yielding to another [Coro] type until it completes
    pub async fn yield_from_type<C, T>(&self) -> T
    where
        C: AsCoro<Snd = S, Rcv = R, Out = T>,
    {
        C::as_coro_fn(*self).await
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
            // SAFETY: we can only poll this future using a waker wrapping State<S, R> and we only
            // ever access the shared state here or inside of Coro::resume which never execute at
            // the same time.
            let data = unsafe {
                (ctx.waker().data() as *mut () as *mut SharedState<S, R>)
                    .as_mut()
                    .unwrap_unchecked()
                    .r
                    .take()
                    .unwrap_unchecked()
            };

            Poll::Ready(data)
        } else {
            self.polled = true;
            // SAFETY: we can only poll this future using a waker wrapping State<S, R> and we only
            // ever access the shared state here or inside of Coro::resume which never execute at
            // the same time.
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{self, Cursor, ErrorKind};
    use std::{sync::mpsc::channel, thread::spawn};

    fn tokio_boom() -> ReadyCoro<(), &'static str, impl Future<Output = &'static str>, ()> {
        Coro::from(async |_: Handle<()>| {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;

            "boom!"
        })
    }

    #[tokio::test]
    #[should_panic = "a Coro is not permitted to await a future that uses an arbitrary Waker"]
    async fn awaiting_a_future_that_needs_a_waker_panics() {
        let _ = tokio_boom().resume();
    }

    // This is handled on the tokio side by checking if the tokio runtime is present but tested
    // here to document the difference in the error message received. In both cases (this and the
    // above) awaiting a future that needs a Waker results in a panic.
    #[test]
    #[should_panic = "there is no reactor running, must be called from the context of a Tokio 1.x runtime"]
    fn awaiting_a_tokio_future_panics_outside_of_tokio() {
        let _ = tokio_boom().resume();
    }

    fn yield_recv_return()
    -> ReadyCoro<bool, &'static str, impl Future<Output = &'static str>, usize> {
        Coro::from(async |handle: Handle<usize, bool>| {
            assert!(handle.yield_value(42).await);

            "hello, world!"
        })
    }

    #[test]
    fn yield_recv_return_works() {
        let mut coro = yield_recv_return();
        coro = coro.resume().unwrap_pending(|n| {
            assert_eq!(n, 42);
            true
        });

        let s = coro.resume().unwrap();
        assert_eq!(s, "hello, world!");
    }

    // This is pretty contrived but worthwhile validating that chaining the unwrap and
    // unwrap_pending methods works fine
    #[test]
    fn coro_state_chaining_works() {
        let s = yield_recv_return()
            .resume()
            .unwrap_pending(|n| {
                assert_eq!(n, 42);
                true
            })
            .resume()
            .unwrap();

        assert_eq!(s, "hello, world!");
    }

    #[test]
    #[should_panic = "called `CoroState::unwrap` on a `Pending` value"]
    fn unwrap_on_pending_panics() {
        yield_recv_return().resume().unwrap();
    }

    #[test]
    #[should_panic = "called `CoroState::unwrap_pending` on a `Complete` value"]
    fn unwrap_pending_on_complete_panics() {
        let mut coro = yield_recv_return();
        coro = coro.resume().unwrap_pending(|n| {
            assert_eq!(n, 42);
            true
        });

        coro.resume().unwrap_pending(|n| {
            assert_eq!(n, 42);
            true
        });
    }

    // Trying to write double_nums as a closure that returns a Coro results in lifetime errors
    // around the lifetime of the coro being longer than the lifetime of the nums reference
    fn double_nums(
        nums: &[usize],
    ) -> ReadyCoro<usize, &'static str, impl Future<Output = &'static str>, usize> {
        Coro::from(async |handle: Handle<usize, usize>| {
            for &n in nums.iter() {
                let doubled = handle.yield_value(n).await;
                assert_eq!(doubled, n * 2);
            }

            "done"
        })
    }

    #[test]
    fn closures_capturing_references_work() {
        let mut coro = double_nums(&[1, 2, 3]);
        loop {
            coro = match coro.resume() {
                CoroState::Pending(c, n) => c.send(n * 2),
                CoroState::Complete(res) => {
                    assert_eq!(res, "done");
                    return;
                }
            };
        }
    }

    #[test]
    fn run_sync_works() {
        let res = double_nums(&[1, 2, 3]).run_sync(|n| n * 2);
        assert_eq!(res, "done");
    }

    struct Echo<T> {
        initial: T,
    }

    impl<T: Unpin + 'static> IntoCoro for Echo<T> {
        type Snd = T;
        type Rcv = T;
        type Out = ();

        async fn into_coro_fn(self, handle: Handle<T, T>) {
            let mut val = self.initial;
            loop {
                val = handle.yield_value(val).await;
            }
        }
    }

    // Contrived but checking that we are able to pass a Coro between threads and resume it without
    // things exploding in any way. Essentially this is a test that when we have Send Coro that's
    // actually valid.
    #[test]
    fn moving_between_threads_works() {
        let mut ping_pong = Echo { initial: "ping" }.into_coro();
        let (tx1, rx1) = channel();
        let (tx2, rx2) = channel();

        let jh1 = spawn(move || {
            for _ in 0..3 {
                ping_pong = match ping_pong.resume() {
                    CoroState::Pending(c, s) => {
                        assert_eq!(s, "ping");
                        let coro = c.send("pong");
                        tx1.send(coro).unwrap();
                        rx2.recv().unwrap()
                    }
                    CoroState::Complete(_) => break,
                };
            }
        });

        let jh2 = spawn(move || {
            for _ in 0..3 {
                let ping_pong = rx1.recv().unwrap();
                match ping_pong.resume() {
                    CoroState::Pending(c, s) => {
                        assert_eq!(s, "pong");
                        let coro = c.send("ping");
                        tx2.send(coro).unwrap();
                    }
                    CoroState::Complete(_) => break,
                }
            }
        });

        jh1.join().unwrap();
        jh2.join().unwrap();
    }

    // ["Hello", "世界"] in 9p wire format.
    const HELLO_WORLD: [u8; 17] = [
        0x02, 0x00, 0x05, 0x00, 0x48, 0x65, 0x6c, 0x6c, 0x6f, 0x06, 0x00, 0xe4, 0xb8, 0x96, 0xe7,
        0x95, 0x8c,
    ];

    async fn read_9p_u16(handle: Handle<usize, Vec<u8>>) -> io::Result<u16> {
        let n = size_of::<u16>();
        let buf = handle.yield_value(n).await;
        let data = buf[0..n].try_into().unwrap();

        Ok(u16::from_le_bytes(data))
    }

    async fn read_9p_string(handle: Handle<usize, Vec<u8>>) -> io::Result<String> {
        let len = handle.yield_from(read_9p_u16).await? as usize;
        let buf = handle.yield_value(len).await;

        String::from_utf8(buf).map_err(|e| io::Error::new(ErrorKind::InvalidData, e.to_string()))
    }

    async fn read_9p_string_vec(handle: Handle<usize, Vec<u8>>) -> io::Result<Vec<String>> {
        let len = handle.yield_from(read_9p_u16).await? as usize;
        let mut buf = Vec::with_capacity(len);
        for _ in 0..len {
            buf.push(handle.yield_from(read_9p_string).await?);
        }

        Ok(buf)
    }

    #[tokio::test]
    async fn nested_yield_from_works() {
        use std::io::Read;

        let mut coro = Coro::from(read_9p_string_vec);
        let mut r = Cursor::new(HELLO_WORLD.to_vec());

        loop {
            coro = match coro.resume() {
                CoroState::Complete(parsed) => {
                    assert_eq!(parsed.unwrap(), &["Hello", "世界"]);
                    return;
                }
                CoroState::Pending(c, n) => c.send({
                    let mut buf = vec![0; n];
                    r.read_exact(&mut buf).unwrap();
                    buf
                }),
            };
        }
    }

    #[tokio::test]
    async fn nested_yield_from_works_with_async() {
        use tokio::io::AsyncReadExt;

        let mut coro = Coro::from(read_9p_string_vec);
        let mut r = Cursor::new(HELLO_WORLD.to_vec());

        loop {
            coro = match coro.resume() {
                CoroState::Complete(parsed) => {
                    assert_eq!(parsed.unwrap(), &["Hello", "世界"]);
                    return;
                }
                CoroState::Pending(c, n) => c.send({
                    let mut buf = vec![0; n];
                    r.read_exact(&mut buf).await.unwrap();
                    buf
                }),
            };
        }
    }

    #[test]
    fn run_sync_works_with_nested_yields() {
        use std::io::Read;

        let mut r = Cursor::new(HELLO_WORLD.to_vec());
        let parsed = Coro::from(read_9p_string_vec)
            .run_sync(|n| {
                let mut buf = vec![0; n];
                r.read_exact(&mut buf).unwrap();
                buf
            })
            .expect("parsing to be successful");

        assert_eq!(parsed, &["Hello", "世界"]);
    }

    // Using const generics to implement a counter generator
    struct Counter<const N: usize>;

    impl<const N: usize> AsCoro for Counter<N> {
        type Snd = usize;
        type Rcv = ();
        type Out = ();

        async fn as_coro_fn(handle: Handle<usize>) {
            for n in 0..N {
                handle.yield_value(n).await
            }
        }
    }

    #[test]
    fn generator_iter_works() {
        let nums: Vec<usize> = Counter::<6>::as_coro().collect();
        assert_eq!(nums, vec![0, 1, 2, 3, 4, 5]);
    }

    #[test]
    fn yield_from_type_works() {
        let coro = Coro::from(async |handle: Handle<usize>| {
            handle.yield_from_type::<Counter<6>, _>().await
        });
        let total: usize = coro.sum();

        assert_eq!(total, 1 + 2 + 3 + 4 + 5);
    }

    // This _doesn't_ work when you are closing over a reference (like in the double_nums case
    // above) due to the Coro capturing a lifetime. Need to work out what is different when you
    // define it as a free function as that works...
    #[test]
    fn capturing_nested_closure_works() {
        let g = |k: usize| {
            Generator::from(move |handle: Handle<usize>| async move {
                for n in 0..k {
                    handle.yield_value(n).await;
                }
            })
        };

        let nums: Vec<usize> = g(6).collect();
        assert_eq!(nums, vec![0, 1, 2, 3, 4, 5]);
    }
}
