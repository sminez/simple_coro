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
pub type CoroFn<S, R, F> = fn(Handle<S, R>) -> F;

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

    /// Provide a [CoroFn] to run as a [Coro].
    fn into_coro_fn(self) -> CoroFn<Self::Snd, Self::Rcv, impl Future<Output = Self::Out>>;

    /// Initialize a new [Coro] from a value.
    fn initialize(
        self,
    ) -> ReadyCoro<Self::Rcv, Self::Out, impl Future<Output = Self::Out>, Self::Snd> {
        Coro {
            _lifecyle: Ready,
            state: SharedState::default(),
            fut: Box::pin((self.into_coro_fn())(Handle {
                _snd: PhantomData,
                _rcv: PhantomData,
            })),
        }
    }
}

// Closures with the correct signature can be directly converted into a [Coro].
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
    F: Future<Output = O>,
    L: Lifecycle,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Coro")
            .field("lifecycle", &self._lifecyle)
            .finish()
    }
}

const DENY_FUT: &str = "a Coro is not permitted to await a future that uses an arbitrary Waker";
const WAKER_VTABLE: RawWakerVTable = RawWakerVTable::new(
    |_| panic!("{DENY_FUT}"),
    |_| panic!("{DENY_FUT}"),
    |_| panic!("{DENY_FUT}"),
    |_| {},
);

impl<R, O, Fut, S> Coro<R, O, Fut, Ready, S>
where
    R: Unpin + 'static,
    S: Unpin + 'static,
    Fut: Future<Output = O>,
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
                let s = self.state.s.take().unwrap_or_else(|| panic!("{DENY_FUT}"));
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

/// The intermediate state of a [AsCoro] while it is executing
#[derive(Debug)]
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
pub struct Handle<S, R = ()>
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
        F: Future<Output = T>,
    {
        coro.into().fut.await
    }

    /// Defer yielding to another [Coro] type until it completes
    pub async fn yield_from_type<T, C, F>(&self) -> T
    where
        C: AsCoro<Snd = S, Rcv = R, Out = T>,
        F: Future<Output = T>,
    {
        C::as_coro(*self).await
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{self, Cursor, ErrorKind};
    use std::{sync::mpsc::channel, thread::spawn};

    #[test]
    fn yield_recv_return_works() {
        let mut coro = Coro::from(async |handle: Handle<usize, bool>| {
            assert!(handle.yield_value(42).await);

            "hello, world!"
        });

        coro = match coro.resume() {
            CoroState::Complete(s) => panic!("first resume should yield, got {s}"),
            CoroState::Pending(c, n) => {
                assert_eq!(n, 42);
                c.send(true)
            }
        };

        match coro.resume() {
            CoroState::Complete(s) => assert_eq!(s, "hello, world!"),
            CoroState::Pending(_, n) => panic!("second resume should complete, got {n}"),
        };
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

    // let double_nums = |nums: &[usize]| {
    //     Coro::from(move |handle: Handle<usize, usize>| async move {
    //         for &n in nums.iter() {
    //             let doubled = handle.yield_value(n).await;
    //             assert_eq!(doubled, n * 2);
    //         }

    //         "done"
    //     })
    // };

    // error: lifetime may not live long enough
    //    --> src/lib.rs:623:13
    //     |
    // 622 |           let double_nums = |nums: &[usize]| {
    //     |                                -       - return type of closure `Coro<usize, &str, {async block@src/lib.rs:623:60: 623:70}, Ready, usize>` contains a lifetime `'2`
    //     |                                |
    //     |                                let's call the lifetime of this reference `'1`
    // 623 | /             Coro::from(move |handle: Handle<usize, usize>| async move {
    // 624 | |                 for &n in nums.iter() {
    // 625 | |                     let doubled = handle.yield_value(n).await;
    // 626 | |                     assert_eq!(doubled, n * 2);
    // ...   |
    // 629 | |                 "done"
    // 630 | |             })
    //     | |______________^ returning this value requires that `'1` must outlive `'2`

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

    // Contrived but checking that we are able to pass a Coro between threads and resume it without
    // things exploding in any way. Essentially this is a test that when we have Send Coro that's
    // actually valid.
    #[test]
    fn moving_between_threads_works() {
        let mut ping_pong = Coro::from(async |handle: Handle<&'static str, &'static str>| {
            let mut s = "ping";
            for _ in 0..10 {
                s = handle.yield_value(s).await;
            }
        });

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

    /// This is the form needed for moving a value into a CoroFn it seems
    fn count(k: usize) -> Generator<usize, impl Future<Output = ()>> {
        Generator::from(move |handle: Handle<usize>| async move {
            for n in 0..k {
                handle.yield_value(n).await;
            }
        })
    }

    #[test]
    fn generator_iter_works() {
        let nums: Vec<usize> = count(6).collect();
        assert_eq!(nums, vec![0, 1, 2, 3, 4, 5]);
    }

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
