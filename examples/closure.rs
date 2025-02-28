//! Testing out support for closures
use crimes::{Coro, Handle, ReadyCoro, Step};

fn double_nums(
    nums: &[usize],
) -> ReadyCoro<usize, &'static str, impl Future<Output = &'static str>, usize> {
    Coro::from(async |handle: Handle<usize, usize>| {
        for &n in nums.iter() {
            println!("  requesting that {n} gets doubled...");
            let doubled = handle.yield_value(n).await;
            println!("  2 x {n} = {doubled}");
        }

        "done"
    })
}

fn main() {
    println!("intializing coroutine");
    let mut state_machine = double_nums(&[1, 2, 3]);

    loop {
        state_machine = {
            match state_machine.step() {
                Step::Pending(sm, n) => {
                    println!("doubling {n}");
                    sm.send(n * 2)
                }

                Step::Complete(res) => {
                    println!("state machine finished with result={res:?}");
                    break;
                }
            }
        };
    }
}
