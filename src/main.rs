#![feature(async_await)]

use futures::channel::oneshot;
use futures::future::{self, Either};
use futures::prelude::*;
use futures::stream::FuturesUnordered;
use rand::prelude::*;
use std::time::{Duration, Instant};

const NEXT_ATTEMPT_WAIT_MILLIS: u64 = 250;
const MAX_ATTEMPTS: usize = 3;

fn t_millis(start: Instant) -> u128 {
    (Instant::now() - start).as_millis()
}

// Simulate a "DNS lookup" (really any kind of network IO, who cares what) by
// just waiting for some random number of milliseconds, flipping a coin, and
// then returning either success or failure.
async fn dummy_dns_lookup(id: usize, start: Instant) -> Result<(), ()> {
    let mut rng = rand::thread_rng();
    let sleep_millis = rng.gen_range(0, 1000);
    tokio_timer::sleep(Duration::from_millis(sleep_millis)).await;
    let succeeds: bool = rng.gen();
    if succeeds {
        println!("{} succeeded at {}ms", id, t_millis(start));
        Ok(())
    } else {
        println!("{} failed at {}ms", id, t_millis(start));
        Err(())
    }
}

async fn attempt_dns(
    id: usize,
    start: Instant,
    fail_sender: oneshot::Sender<()>,
) -> Result<(), ()> {
    let result = dummy_dns_lookup(id, start).await;
    if result.is_err() {
        // We don't care if the receiver is gone.
        let _ = fail_sender.send(());
    }
    result
}

#[tokio::main]
async fn main() {
    let start = Instant::now();
    let mut unordered = FuturesUnordered::new();
    let mut id = 0;
    // Until we hit the maximum number of attempts, spawn an attempt each time
    // through this loop.
    while id < MAX_ATTEMPTS {
        let (fail_sender, fail_receiver) = oneshot::channel::<()>();
        let timeout = tokio_timer::sleep(Duration::from_millis(NEXT_ATTEMPT_WAIT_MILLIS));
        let mut next_attempt = future::select(fail_receiver, timeout);
        unordered.push(attempt_dns(id, start, fail_sender));
        println!("began attempt {} at {}ms", id, t_millis(start));
        id += 1;
        // Keep waiting until it's time to fire the next attempt.
        loop {
            match future::select(unordered.next(), &mut next_attempt).await {
                Either::Left((Some(Ok(())), _)) => {
                    // A request succeeded. We're done!
                    return;
                }
                Either::Left((Some(Err(())), _)) => {
                    // A request failed. Just keep waiting.
                }
                Either::Left((None, _)) | Either::Right(_) => {
                    // Either the most recent task failed or the timeout fired.
                    // Time to kick off the next attempt! Break out of this
                    // inner loop.
                    break;
                }
            }
        }
    }
    println!("Max attempts reached.");
    // Now as long as there are tasks in flight, keep awaiting them.
    while let Some(result) = unordered.next().await {
        match result {
            Ok(()) => {
                // A request succeeded. We're done!
                return;
            }
            Err(()) => {
                // A request failed. Just keep waiting.
            }
        }
    }
    println!("Finished without success.");
}
