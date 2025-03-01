use crimes::{Coro, Handle, ReadyCoro, Step};

#[test]
fn coro_from_closure_works() {
    let mut coro = Coro::from(async |handle: Handle<&'static str, &'static str>| {
        assert_eq!("pong", handle.yield_value("ping").await);
    });

    coro = match coro.step() {
        Step::Pending(c, ping) => {
            assert_eq!(ping, "ping");
            c.send("pong")
        }
        Step::Complete(()) => panic!("expected pending"),
    };

    match coro.step() {
        Step::Pending(_, _) => panic!("expected complete"),
        Step::Complete(()) => (),
    };
}

#[test]
fn closures_capturing_referece_work() {
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

    let mut coro = double_nums(&[1, 2, 3]);
    loop {
        coro = {
            match coro.step() {
                Step::Pending(c, n) => c.send(n * 2),
                Step::Complete(res) => {
                    assert_eq!(res, "done");
                    return;
                }
            }
        };
    }
}
