//! Behaviour involving threads
use crimes::{Coro, CoroState, Handle};
use std::{sync::mpsc::channel, thread::spawn};

// Contrived but checking that we are able to pass a Coro between threads and resume it without
// things exploding in any way.
#[test]
fn moving_between_threads() {
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
            ping_pong = {
                match ping_pong.resume() {
                    CoroState::Pending(c, s) => {
                        assert_eq!(s, "ping");
                        let coro = c.send("pong");
                        tx1.send(coro).unwrap();
                        rx2.recv().unwrap()
                    }
                    CoroState::Complete(_) => break,
                }
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
