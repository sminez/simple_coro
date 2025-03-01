use crimes::{Coro, Handle, Step};
use std::io::{self, Cursor, ErrorKind, Read};

// ["Hello", "世界"] in 9p wire format.
const HELLO_WORLD: [u8; 17] = [
    0x02, 0x00, 0x05, 0x00, 0x48, 0x65, 0x6c, 0x6c, 0x6f, 0x06, 0x00, 0xe4, 0xb8, 0x96, 0xe7, 0x95,
    0x8c,
];

#[test]
fn nested_yield_from_works() {
    let mut r = Cursor::new(HELLO_WORLD.to_vec());
    let mut coro = Coro::from(read_9p_string_vec);
    loop {
        coro = {
            match coro.step() {
                Step::Pending(c, n) => {
                    let mut buf = vec![0; n];
                    r.read_exact(&mut buf).unwrap();
                    c.send(buf)
                }

                Step::Complete(Err(e)) => panic!("parsing failed: {e}"),

                Step::Complete(Ok(parsed)) => {
                    assert_eq!(parsed, &["Hello", "世界"]);
                    return;
                }
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
