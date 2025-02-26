use std::{
    future::Future,
    pin::{Pin, pin},
    sync::Arc,
    task::{Context, Poll, Wake, Waker},
};

struct Data(&'static str);

// We don't actually need to handle things correctly here so lets just no-op
impl Wake for Data {
    fn wake(self: Arc<Self>) {}
    fn wake_by_ref(self: &Arc<Self>) {}
}

struct SneakyFuture;
impl Future for SneakyFuture {
    type Output = &'static str;

    fn poll(self: Pin<&mut Self>, ctx: &mut Context<'_>) -> Poll<Self::Output> {
        // SAFETY: highly questionable
        let data = unsafe { (ctx.waker().data() as *const Data).as_ref().unwrap().0 };

        Poll::Ready(data)
    }
}

struct ReallySneakyFuture;
impl Future for ReallySneakyFuture {
    type Output = ();

    fn poll(self: Pin<&mut Self>, ctx: &mut Context<'_>) -> Poll<Self::Output> {
        // SAFETY: even more highly questionable
        unsafe {
            (ctx.waker().data() as *mut () as *mut Data)
                .as_mut()
                .unwrap()
                .0 = "psych! new data!"
        };

        Poll::Ready(())
    }
}

fn main() {
    let state = Arc::new(Data("Hello, world!"));
    let waker = Waker::from(state.clone());
    let mut ctx = Context::from_waker(&waker);

    match pin!(SneakyFuture).poll(&mut ctx) {
        Poll::Pending => panic!("should only return ready"),
        Poll::Ready(data) => println!("got some data -> {data}"),
    }

    match pin!(ReallySneakyFuture).poll(&mut ctx) {
        Poll::Pending => panic!("should only return ready"),
        Poll::Ready(_) => println!("did sneaky things..."),
    }

    println!("contents of state = {}", state.0);
}
