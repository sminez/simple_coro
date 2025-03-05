# Simple Coro

(ab)using Rust's async/await syntax to write simple state-machine based coroutines.

> :warning: Formally known as [crimes][0] (see below) :warning:


## Why is this criminal?

Originally the "crimes" aspect of this was due to it being a quickly hacked
together proof of concept that had a safe(ish) API. As I've continued to work
on things it's beginning to look more and more reasonable but I'd still argue
that this is probably _not_ what you want, particularly if you are happy using
the nightly compiler support for [coroutines][1].

## A note on API changes from the blog post

After writing my [Socrates is a state machine][2] blog post I couldn't resist
trying to simplify the API, so rather than what is presented there (a paired
runner and state machine) the updated API now handles everything with a single
struct: `Coro`. It also enforces that replies are sent before calling `resume`
again via a lifecycle typestate and provides a number of quality of life
functionality that simplifies writing simple coroutines.

To take a look through the API as it was when I wrote the blog post you'll need
to go [here][3].

```rust
use simple_coro::{Coro, CoroState, Handle};
use std::io::{self, Cursor, ErrorKind};

// ["Hello", "世界"] in 9p wire format.
const HELLO_WORLD: [u8; 17] = [
    0x02, 0x00, 0x05, 0x00, 0x48, 0x65, 0x6c, 0x6c, 0x6f, 0x06, 0x00, 0xe4, 0xb8, 0x96, 0xe7, 0x95,
    0x8c,
];

#[tokio::main]
async fn main() {
    parse_sync();
    parse_async().await;
}

fn parse_sync() {
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

async fn parse_async() {
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
```

  [0]: https://www.sminez.dev/socrates-is-a-state-machine/#the-rest-of-the-owl
  [1]: https://doc.rust-lang.org/std/ops/trait.Coroutine.html
  [2]: https://www.sminez.dev/socrates-is-a-state-machine/
  [3]: https://github.com/sminez/crimes/tree/1ea8a028f861b7d6061f3153af5532fc77856058
