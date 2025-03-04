//! Testing out support for closures
use crimes::{Coro, Handle, ReadyCoro};

fn double_nums(
    nums: &[usize],
) -> ReadyCoro<usize, usize, &'static str, impl Future<Output = &'static str>> {
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
    let coro = double_nums(&[1, 2, 3]);
    let res = coro.run_sync(|n| {
        println!("doubling {n}");
        n * 2
    });
    println!("state machine finished with result={res:?}");
}
