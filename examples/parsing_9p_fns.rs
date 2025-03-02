//! An example of how to use crimes to parse the 9p protocol wire format using free async functions
use crimes::{Coro, CoroFn, CoroState, Handle, Ready};
use std::{
    future::Future,
    io::{self, Cursor, ErrorKind, Read},
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
    let mut state_machine = T::init();
    loop {
        state_machine = match state_machine.resume() {
            CoroState::Complete(res) => return res,
            CoroState::Pending(sm, n) => {
                println!("{n} bytes requested");
                let mut buf = vec![0; n];
                r.read_exact(&mut buf)?;
                sm.send(buf)
            }
        };
    }
}

/// Read a [Read9p] value using an implementation of [AsyncRead] to perform the required IO.
async fn read_9p_async_from_bytes<T, R>(r: &mut R) -> io::Result<T>
where
    T: Read9p,
    R: AsyncRead + Unpin,
{
    let mut state_machine = T::init();
    loop {
        state_machine = match state_machine.resume() {
            CoroState::Complete(res) => return res,
            CoroState::Pending(sm, n) => {
                println!("{n} bytes requested");
                let mut buf = vec![0; n];
                r.read_exact(&mut buf).await?;
                sm.send(buf)
            }
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
    let len = (u16::gen_fn())(handle).await? as usize;
    let buf = handle.yield_value(len).await;

    String::from_utf8(buf).map_err(|e| io::Error::new(ErrorKind::InvalidData, e.to_string()))
}

async fn read_9p_vec<T: Read9p>(handle: Handle<usize, Vec<u8>>) -> io::Result<Vec<T>> {
    let len = (u16::gen_fn())(handle).await? as usize;
    let mut buf = Vec::with_capacity(len);
    for _ in 0..len {
        buf.push((T::gen_fn())(handle).await?);
    }

    Ok(buf)
}

trait Read9p: Sized {
    fn gen_fn() -> CoroFn<usize, Vec<u8>, impl Future<Output = io::Result<Self>> + Send>;

    fn init()
    -> Coro<Vec<u8>, io::Result<Self>, impl Future<Output = io::Result<Self>> + Send, Ready, usize>
    {
        Self::gen_fn().into()
    }
}

macro_rules! impl_read9p {
    ($ty:ty, $fn:ident) => {
        impl Read9p for $ty {
            fn gen_fn() -> CoroFn<usize, Vec<u8>, impl Future<Output = io::Result<Self>> + Send> {
                $fn
            }
        }
    };
    ($bound:ident, $ty:ty, $fn:ident) => {
        impl<$bound: Read9p + Send> Read9p for $ty {
            fn gen_fn() -> CoroFn<usize, Vec<u8>, impl Future<Output = io::Result<Self>> + Send> {
                $fn
            }
        }
    };
}

impl_read9p!(u16, read_9p_u16);
impl_read9p!(String, read_9p_string);
impl_read9p!(T, Vec<T>, read_9p_vec);
