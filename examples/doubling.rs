use std::{
    future::Future,
    pin::{Pin, pin},
    sync::{Arc, Mutex},
    task::{Context, Poll, Wake, Waker},
};

// Now we need to store a usize instead of a string and we need to be able to
// mutate that usize from the poll loop
struct Data(Mutex<usize>);

// We don't actually need to handle things correctly here so lets just no-op
impl Wake for Data {
    fn wake(self: Arc<Self>) {}
    fn wake_by_ref(self: &Arc<Self>) {}
}

struct Yield {
    polled: bool,
    n: usize,
}

impl Future for Yield {
    type Output = usize;

    fn poll(mut self: Pin<&mut Self>, ctx: &mut Context<'_>) -> Poll<usize> {
        if self.polled {
            // SAFETY: we can only poll this future using a waker wrapping Data
            let data = unsafe {
                *(ctx.waker().data() as *mut () as *mut Data)
                    .as_mut()
                    .unwrap()
                    .0
                    .lock()
                    .unwrap()
            };

            Poll::Ready(data)
        } else {
            self.polled = true;
            // SAFETY: we can only poll this future using a waker wrapping Data
            unsafe {
                *(ctx.waker().data() as *mut () as *mut Data)
                    .as_mut()
                    .unwrap()
                    .0
                    .lock()
                    .unwrap() = self.n;
            };

            Poll::Pending
        }
    }
}

async fn request_doubling(nums: &[usize]) -> &'static str {
    for &n in nums.iter() {
        println!("  requesting that {n} gets doubled...");
        let doubled = Yield { polled: false, n }.await;
        println!("  2 x {n} = {doubled}");
    }

    "done"
}

fn main() {
    let state = Arc::new(Data(Mutex::new(0))); // starting value doesn't matter
    let waker = Waker::from(state.clone());
    let mut ctx = Context::from_waker(&waker);

    println!("creating future");
    let mut fut = pin!(request_doubling(&[1, 2, 3]));

    loop {
        println!("polling");
        match fut.as_mut().poll(&mut ctx) {
            Poll::Pending => {
                let n = *state.0.lock().unwrap();
                println!("setting result");
                *state.0.lock().unwrap() = 2 * n;
            }

            Poll::Ready(data) => {
                println!("future finished with result={data}");
                break;
            }
        }
    }
}
