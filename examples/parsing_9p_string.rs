// "use crimes" was too good to pass up
use crimes::{Handle, RunState, Runner, StateMachine, Step};
use std::{io, marker::PhantomData};
use tokio::io::{AsyncRead, AsyncReadExt};

// The string "Hello, 世界" encoded in 9p binary format
const HELLO_WORLD: [u8; 15] = [
    0x0d, 0x00, 0x48, 0x65, 0x6c, 0x6c, 0x6f, 0x2c, 0x20, 0xe4, 0xb8, 0x96, 0xe7, 0x95, 0x8c,
];

#[tokio::main]
async fn main() -> io::Result<()> {
    println!(">> reading using std::io::Read");
    let s: String = read_9p_sync_from_bytes(&mut io::Cursor::new(HELLO_WORLD.to_vec()))?;
    println!("  got val: {s:?}\n");

    println!(">> reading using tokio::io::AsyncRead");
    let s: String = read_9p_async_from_bytes(&mut io::Cursor::new(HELLO_WORLD.to_vec())).await?;
    println!("  got val: {s:?}");

    Ok(())
}

// This is what our blocking I/O read loop ends up looking like
fn read_9p_sync_from_bytes<T: Read9p, R: io::Read>(r: &mut R) -> io::Result<T> {
    let runner = Runner::new(NinepState);
    let mut state_machine = runner.init::<NineP<T>>();
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

// This is what our non-blocking I/O read loop ends up looking like
async fn read_9p_async_from_bytes<T: Read9p, R: AsyncRead + Unpin>(r: &mut R) -> io::Result<T> {
    let runner = Runner::new(NinepState);
    let mut state_machine = runner.init::<NineP<T>>();
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

// We don't need anything special for our run state so we can just use an empty struct
struct NinepState;
impl RunState for NinepState {
    type Snd = usize;
    type Rcv = Vec<u8>;
}

// Defining a wrapper trait lets us implement it directly on the types we care about even if we
// don't own them
trait Read9p: Sized {
    fn read_9p(handle: Handle<usize, Vec<u8>>) -> impl Future<Output = io::Result<Self>> + Send;
}

// But when it comes to implementing StateMachine, we need to use a wrapper type to avoid the
// orphan rule
struct NineP<T>(PhantomData<T>);
impl<T: Read9p> StateMachine for NineP<T> {
    type Snd = usize;
    type Rcv = Vec<u8>;
    type Out = io::Result<T>;

    async fn run(handle: Handle<usize, Vec<u8>>) -> io::Result<T> {
        T::read_9p(handle).await
    }
}

impl Read9p for u16 {
    async fn read_9p(handle: Handle<usize, Vec<u8>>) -> io::Result<u16> {
        let n = size_of::<u16>();
        let buf = handle.yield_value(n).await;
        let data = buf[0..n].try_into().unwrap();
        Ok(u16::from_le_bytes(data))
    }
}

impl Read9p for String {
    async fn read_9p(handle: Handle<usize, Vec<u8>>) -> io::Result<String> {
        let len = NineP::<u16>::run(handle).await? as usize;
        let buf = handle.yield_value(len).await;
        String::from_utf8(buf)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))
    }
}
