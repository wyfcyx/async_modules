use std::task::Poll;
use std::task::{Context, RawWaker, RawWakerVTable, Waker};
use std::future::Future;
extern crate alloc;
use alloc::boxed::Box;
use std::pin::Pin;
use std::sync::Mutex;
use lazy_static::*;
use std::sync::mpsc::{channel, Sender};
use std::time::Duration;

mod modules;

async fn hello() -> usize {
    println!("into hello");
    let f = TestFuture::new();
    let r = f.await;
    println!("r = {}", r);
    100
}

struct TestFuture {
    accessed: bool,
}

impl TestFuture {
    pub fn new() -> Self {
        Self { accessed: false }
    }
}

impl Future for TestFuture {
    type Output = usize;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
        if !self.accessed {
            //self.accessed = true;
            unsafe {
                let tx = &mut *(WAKER_PTR as *mut Sender<Waker>);
                let test_waker = Box::new(TestWaker {
                    flag: &mut self.accessed as *mut _
                });
                //std::mem::forget(test_waker);
                let raw_waker = RawWaker::new(
                    Box::into_raw(test_waker) as *const (),
                    &TestWakerVTable,
                );
                tx.send(Waker::from_raw(raw_waker)).expect("Error when sending!");
            }
            Poll::Pending    
        } else {
            Poll::Ready(200)
        }
    }    
}

struct TestWaker {
    flag: *mut bool,
}

const TestWakerVTable: RawWakerVTable = RawWakerVTable::new(
    |data| unsafe {
        RawWaker::new(data, &TestWakerVTable)
    },
    |data| unsafe {
        let mut waker = &mut *(data as *mut TestWaker);
        *(waker.flag) = true;
    },
    |data| unsafe {

    },
    |data| unsafe {
    
    }
);

static mut WAKER_PTR: *mut () = core::ptr::null_mut();

const EmptyWakerVTable: RawWakerVTable = RawWakerVTable::new(
    |data| {RawWaker::new(data, &EmptyWakerVTable)},
    |data| {},
    |data| {},
    |data| {},
);

fn main() {
    let (mut tx, rx) = channel::<Waker>();
    unsafe {
        WAKER_PTR = &mut tx as *mut _ as *mut ();
    }
    let handle = std::thread::spawn(move || {
        let waker = rx.recv().unwrap();
        std::thread::sleep(Duration::from_secs(5));
        println!("wake!");
        waker.wake();
    });
    
    let mut task = Box::pin(hello());
    //let task = boxed.as_mut();
    let raw_waker = RawWaker::new(core::ptr::null(), &EmptyWakerVTable);
    let waker = unsafe { Waker::from_raw(raw_waker) }; 
    let mut cx = Context::from_waker(&waker);
    loop {
        match task.as_mut().poll(&mut cx) {
            Poll::Ready(v) => {
                println!("Ready {}", v);
                break;
            },
            Poll::Pending => {
                //println!("Pending");
            }
        }
    }
    handle.join().unwrap();
    println!("exited");
}
