#![feature(async_await)]

use futures::channel::mpsc;
use futures::channel::oneshot;
use futures::prelude::*;
use futures::stream::{Fuse, FuturesUnordered};
use rand::prelude::*;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};
use tokio::prelude::*;

const WAIT_MILLIS: u64 = 250;
const MAX_ATTEMPTS: u64 = 3;

struct Nursery<T: Future> {
    receiver: Fuse<mpsc::UnboundedReceiver<T>>,
    futures: FuturesUnordered<T>,
}

impl<T: Future> Nursery<T> {
    fn new() -> (Self, mpsc::UnboundedSender<T>) {
        let (sender, receiver) = mpsc::unbounded();
        (
            Self {
                receiver: receiver.fuse(),
                futures: FuturesUnordered::new(),
            },
            sender,
        )
    }
}

impl<T: Future> Stream for Nursery<T> {
    type Item = T::Output;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Option<Self::Item>> {
        // Take as many futures as we can from the receiver and seat them in
        // the FuturesUnordered collection. Note that if we didn't loop until
        // Poll::Pending here, we wouldn't be guaranteed to get another wakeup
        // in the future.
        loop {
            match self.receiver.poll_next_unpin(cx) {
                Poll::Ready(Some(future)) => self.futures.push(future),
                Poll::Ready(None) | Poll::Pending => break,
            }
        }
        // If any futures are ready, return one item to the caller.
        if let Poll::Ready(Some(item)) = self.futures.poll_next_unpin(cx) {
            return Poll::Ready(Some(item));
        }
        // If there are no futures in the collection, and the channel is
        // closed, we're done.
        if self.receiver.is_done() && self.futures.is_empty() {
            Poll::Ready(None)
        } else {
            Poll::Pending
        }
    }
}

// Simulate a "DNS lookup" (really any kind of network IO, who cares what) by
// just waiting for some random number of milliseconds, flipping a coin, and
// then returning either success or failure.
async fn dummy_dns_lookup() -> Result<(), ()> {
    let mut rng = rand::thread_rng();
    let sleep_millis = rng.gen_range(0, 1000);
    tokio_timer::sleep(Duration::from_millis(sleep_millis)).await;
    let succeeds: bool = rng.gen();
    if succeeds {
        Ok(())
    } else {
        Err(())
    }
}

fn log(id: u64, verb: &str, t0: Instant) {
    let millis = (Instant::now() - t0).as_millis();
    eprintln!("{} {} at {} ms", id, verb, millis);
}

async fn attempt(
    id: u64,
    prev_fail_receiver: Option<oneshot::Receiver<()>>,
    mut attempt_sender: mpsc::UnboundedSender<Pin<Box<dyn Future<Output = Result<u64, u64>>>>>,
    t0: Instant,
) -> Result<u64, u64> {
    if let Some(prev_fail) = prev_fail_receiver {
        // Timeout isn't an error here, so ignore the result.
        let _ = prev_fail.timeout(Duration::from_millis(WAIT_MILLIS)).await;
    }
    log(id, "started", t0);
    let (fail_sender, fail_receiver) = oneshot::channel();
    if id + 1 < MAX_ATTEMPTS {
        attempt_sender
            .send(Box::pin(attempt(
                id + 1,
                Some(fail_receiver),
                attempt_sender.clone(),
                t0,
            )))
            .await
            .unwrap();
    } else {
        eprintln!("Reached the maximum number of attempts.");
    }
    match dummy_dns_lookup().await {
        Ok(()) => Ok(id),
        Err(()) => {
            // Ignore failure in this send, because the receiving attempt might
            // have failed already too.
            let _ = fail_sender.send(());
            Err(id)
        }
    }
}

async fn happy_eyeballs() -> Result<(), ()> {
    let t0 = Instant::now();
    let (mut nursery, mut attempt_sender) = Nursery::new();
    attempt_sender
        .send(Box::pin(attempt(0, None, attempt_sender.clone(), t0)))
        .await
        .unwrap();
    drop(attempt_sender);
    loop {
        match nursery.next().await {
            Some(Ok(id)) => {
                log(id, "succeeded", t0);
                return Ok(());
            }
            Some(Err(id)) => {
                log(id, "failed", t0);
            }
            None => {
                eprintln!("All attempts failed.");
                return Err(());
            }
        }
    }
}

#[tokio::main]
async fn main() {
    let _ = happy_eyeballs().await;
}
