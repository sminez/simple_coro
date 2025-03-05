# Crimes - An exploration of (ab)using async/await syntax to simplify writing coroutines

While we wait for the experimental [coroutine API][0] to land, this crate offers a way of writing
ergonomic coroutines on stable Rust today without resorting to proc macros or other code generation
techniques. In order to make this work we need to commit a few crimes by way of abusing the fact
that Rust's `async/await` syntax [de-sugars][1] into a state machine that implements [Future][2]. There
are a couple of (documented) foot guns lying around that you need to be wary of but so long as you
are alright with that you can play around with coroutines today.

> Please note that the top level API provided by this crate is not 100% compatible with the current
> API that is proposed on nightly. 

This crate is [no_std][3] compatible.


## Coroutines from async functions

The [Coro] struct represents a running coroutine which can be driven externally from by calling
[resume][Coro::resume]. The quickest way to create a `Coro` is to build one from an async function
with the appropriate signature:

```rust
use simple_coro::{Coro, CoroState, Handle};

let mut coro = Coro::from(async |handle: Handle<usize, bool>| {
    let say_hello: bool = handle.yield_value(42).await;
    if say_hello {
        "Hello, World!"
    } else {
        "Goodbye cruel world!"
    }
});


loop {
    coro = match coro.resume() {
        CoroState::Pending(c, n) => {
            assert_eq!(n, 42);
            c.send(true)
        }

        CoroState::Complete(msg) => {
            assert_eq!(msg, "Hello, World!");
            break;
        }
    };
}
```

A `Coro` can be made from any async function that accepts a [Handle] as its single argument, optionally
returning a value as in the example here (we call this a [CoroFn]). The `Handle` argument serves several
purposes:

  1. The only way to obtain a `Handle` is through the construction of a `Coro` so you will not be able
     to call this function directly. The (ab)use of `async/await` syntax to make this all work means
     that the `Future` returned by this function can not be awaited under a normal async runtime so we
     need to ensure that it is only called as part of running the `Coro`. It also can't await arbitrary
     futures, only those made available as methods on [Handle] and other `CoroFns`.
  2. The two generic types for a `Handle<S, R>` specify the values that will be Sent and Received from
     yield points.
  3. Communication between a running `Coro` and the code driving it is possible via methods on a `Handle`.

Once we have a [Coro] we are responsible for running it by calling the [resume][Coro::resume] and
[send][Coro::send] methods: this crate does not provide any form of runtime. `resume` takes ownership of
a [Ready] coroutine and gives you back a [CoroState] that either contains the suspended coroutine along with
the value it emitted from its next [yield][Handle::yield_value], or the return value of the coroutine if
it has finished. `send` takes ownership of a [Pending] coroutine and stores a response to the last `yield`
for the coroutine to receive when it is resumed.

The [run_sync][Coro::run_sync] method on `Coro` simplifies writing the loop construct in the example
above if you always want to respond to yields synchronously in the same way, and the [unwrap][CoroState::unwrap]
and [unwrap_pending][CoroState::unwrap_pending] methods on `CoroState` let you avoid having to match if
you know the state you coming back from a particular yield.

## Implementing Coroutines explicitly

If you want to implement a coroutine directly then you can implement either of the [AsCoro] or [IntoCoro]
traits, depending on whether you need to make use of state held within the implementor or not.

  [0]: https://doc.rust-lang.org/std/ops/trait.Coroutine.html
  [1]: https://doc.rust-lang.org/std/future/trait.IntoFuture.html#await-desugaring
  [2]: https://doc.rust-lang.org/std/future/trait.Future.html
  [3]: https://github.com/rust-lang/rfcs/blob/master/text/1184-stabilize-no_std.md
