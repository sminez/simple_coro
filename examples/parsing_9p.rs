//! An example of how to use crimes to parse the 9p protocol wire format
use crimes::{Handle, RunState, Runner, StateMachine, Step};
use std::{
    future::Future,
    io::{self, Cursor, ErrorKind, Read},
    marker::PhantomData,
};
use tokio::io::{AsyncRead, AsyncReadExt};

// ["Hello", "世界"] in 9p wire format.
//
// In 9p, data items of larger or variable lengths are represented by a two-byte field
// specifying a count, n, followed by n bytes of data.
// - Strings are represented this way with the data stored as UTF-8 without a trailing null byte.
// - Arrays are represented as a length for the sum of the encoded data for all elements followed
//   by the encoded form of each element.
const HELLO_WORLD: [u8; 17] = [
    0x02, 0x00, 0x05, 0x00, 0x48, 0x65, 0x6c, 0x6c, 0x6f, 0x06, 0x00, 0xe4, 0xb8, 0x96, 0xe7, 0x95,
    0x8c,
];

#[tokio::main]
async fn main() -> io::Result<()> {
    println!(">> reading using std::io::Read");
    let v: Vec<String> = read_9p_sync_from_bytes(&mut Cursor::new(HELLO_WORLD.to_vec()))?;
    println!("  got val: {v:?}\n");

    println!(">> reading using tokio::io::AsyncRead");
    let v: Vec<String> = read_9p_async_from_bytes(&mut Cursor::new(HELLO_WORLD.to_vec())).await?;
    println!("  got val: {v:?}");

    Ok(())
}

/// Read a [Read9p] value using an implementation of [Read] to perform the required IO.
fn read_9p_sync_from_bytes<T, R>(r: &mut R) -> io::Result<T>
where
    T: Read9p,
    R: Read,
{
    let runner = Runner::new(NinepState);
    let mut state_machine = runner.init::<NinepParser<T>>();

    loop {
        match runner.step(&mut state_machine) {
            Step::Complete(res) => return res,
            Step::Pending(n) => {
                println!("{n} bytes requested");
                let mut buf = vec![0; n];
                r.read_exact(&mut buf)?;
                runner.send(buf);
            }
        }
    }
}

/// Read a [Read9p] value using an implementation of [AsyncRead] to perform the required IO.
async fn read_9p_async_from_bytes<T, R>(r: &mut R) -> io::Result<T>
where
    T: Read9p,
    R: AsyncRead + Unpin,
{
    let runner = Runner::new(NinepState);
    let mut state_machine = runner.init::<NinepParser<T>>();

    loop {
        match runner.step(&mut state_machine) {
            Step::Complete(res) => return res,
            Step::Pending(n) => {
                println!("{n} bytes requested");
                let mut buf = vec![0; n];
                r.read_exact(&mut buf).await?;
                runner.send(buf);
            }
        }
    }
}

/// An impl of [RunState] for running 9p parsing [StateMachine]s
#[derive(Default)]
struct NinepState;
impl RunState for NinepState {
    type Snd = usize;
    type Rcv = Vec<u8>;
}

/// A type that can be deserialized from 9p wire format
trait Read9p: Sized {
    fn read_9p(handle: Handle<usize, Vec<u8>>) -> impl Future<Output = io::Result<Self>> + Send;
}

/// In order to provide a generic impl of [StateMachine] for [Read9p] we need to wrap things in
/// a new type to avoid hitting the orphan rule. The alternative is to just use the [StateMachine]
/// trait directly and specify the Snd and Rcv types every time.
struct NinepParser<T>(PhantomData<T>)
where
    T: Read9p;

impl<T> StateMachine for NinepParser<T>
where
    T: Read9p,
{
    type Snd = usize;
    type Rcv = Vec<u8>;
    type Out = io::Result<T>;

    async fn run(handle: Handle<Self::Snd, Self::Rcv>) -> Self::Out {
        <T as Read9p>::read_9p(handle).await
    }
}

// Impls for the types we need to decode a Vec<String>

impl Read9p for u16 {
    async fn read_9p(handle: Handle<usize, Vec<u8>>) -> io::Result<Self> {
        let n = size_of::<u16>();
        let buf = handle.yield_value(n).await;
        let data = buf[0..n].try_into().unwrap();

        Ok(u16::from_le_bytes(data))
    }
}

impl Read9p for String {
    async fn read_9p(handle: Handle<usize, Vec<u8>>) -> io::Result<Self> {
        let len = u16::read_9p(handle).await? as usize;
        let buf = handle.yield_value(len).await;

        String::from_utf8(buf).map_err(|e| io::Error::new(ErrorKind::InvalidData, e.to_string()))
    }
}

impl<T: Read9p + Send> Read9p for Vec<T> {
    async fn read_9p(handle: Handle<usize, Vec<u8>>) -> io::Result<Self> {
        let len = u16::read_9p(handle).await? as usize;
        let mut buf = Vec::with_capacity(len);
        for _ in 0..len {
            buf.push(T::read_9p(handle).await?);
        }

        Ok(buf)
    }
}
