use simple_coro::{Coro, CoroState, Handle, ReadyCoro};
use std::{cell::UnsafeCell, io};

/// A wildly unsafe shared mutable reference to a shared byte buffer
pub(crate) struct SharedBuf(UnsafeCell<Vec<u8>>);

impl SharedBuf {
    #[allow(clippy::mut_from_ref)]
    pub fn as_inner_mut(&self) -> &mut Vec<u8> {
        // SAFETY: the caller must guarantee that no concurrent access takes place
        unsafe { &mut *self.0.get() }
    }

    pub fn as_slice(&self) -> &[u8] {
        // SAFETY: the caller must guarantee that no concurrent access takes place
        unsafe { &*self.0.get() }
    }
}

// ["Hello", "世界"] in 9p wire format.
const HELLO_WORLD: [u8; 17] = [
    0x02, 0x00, 0x05, 0x00, 0x48, 0x65, 0x6c, 0x6c, 0x6f, 0x06, 0x00, 0xe4, 0xb8, 0x96, 0xe7, 0x95,
    0x8c,
];

fn main() -> io::Result<()> {
    let v: Vec<String> = read_9p_sync_from_bytes(&mut io::Cursor::new(HELLO_WORLD.to_vec()))?;
    println!("  got val: {v:?}\n");

    Ok(())
}

fn read_9p_sync_from_bytes<T: Read9p, R: io::Read>(r: &mut R) -> io::Result<T> {
    let buf = SharedBuf(UnsafeCell::new(Vec::with_capacity(100)));
    let mut coro = T::coro(&buf);

    loop {
        coro = match coro.resume() {
            CoroState::Complete(res) => return res,
            CoroState::Pending(c, n) => {
                println!("{n} bytes requested");
                let mut_buf = buf.as_inner_mut();
                mut_buf.resize(n, 0);
                r.read_exact(mut_buf)?;
                c.send(())
            }
        };
    }
}

trait Read9p: Sized {
    fn coro_fn(buf: &SharedBuf, handle: Handle<usize>) -> impl Future<Output = io::Result<Self>>;

    fn coro(
        buf: &SharedBuf,
    ) -> ReadyCoro<usize, (), io::Result<Self>, impl Future<Output = io::Result<Self>>> {
        Coro::from(async move |handle: Handle<usize>| Self::coro_fn(buf, handle).await)
    }
}

impl Read9p for u16 {
    async fn coro_fn(buf: &SharedBuf, handle: Handle<usize>) -> io::Result<Self> {
        let n = size_of::<u16>();
        handle.yield_value(n).await;
        let data = buf.as_slice()[0..n].try_into().unwrap();

        Ok(u16::from_le_bytes(data))
    }
}

impl Read9p for String {
    async fn coro_fn(buf: &SharedBuf, handle: Handle<usize>) -> io::Result<Self> {
        let len = u16::coro_fn(buf, handle).await? as usize;
        handle.yield_value(len).await;
        String::from_utf8(buf.as_slice().to_vec())
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))
    }
}

impl<T: Read9p> Read9p for Vec<T> {
    async fn coro_fn(buf: &SharedBuf, handle: Handle<usize>) -> io::Result<Self> {
        let len = u16::coro_fn(buf, handle).await? as usize;
        let mut elems = Vec::with_capacity(len);
        for _ in 0..len {
            elems.push(T::coro_fn(buf, handle).await?);
        }

        Ok(elems)
    }
}
