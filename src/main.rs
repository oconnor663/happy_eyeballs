use async_std::io;
use async_std::task;
use futures::pin_mut;
use futures::prelude::*;
use futures::select;
use futures::stream::FuturesUnordered;
use rand::prelude::*;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::time::{Duration, Instant};
use lazy_static::lazy_static;

const WAIT_MILLIS: u64 = 250;

type Connection = ();

lazy_static! {
    static ref START: Instant = Instant::now();
}

fn log(addr: std::net::SocketAddr, verb: &str) {
    let millis = (Instant::now() - *START).as_millis();
    let last_digit = match addr.ip() {
        std::net::IpAddr::V4(ip) => *ip.octets().last().unwrap(),
        std::net::IpAddr::V6(ip) => *ip.octets().last().unwrap(),
    };
    eprintln!("{} {} at {} ms", last_digit, verb, millis);
}

// Simulate a "DNS lookup" (really any kind of network IO, who cares what) by
// just waiting for some random number of milliseconds, flipping a coin, and
// then returning either success or failure.
async fn dummy_connect(addr: std::net::SocketAddr) -> io::Result<Connection> {
    log(addr, "started");
    let sleep_millis = rand::thread_rng().gen_range(0, 1000);
    task::sleep(Duration::from_millis(sleep_millis)).await;
    let succeeds: bool = rand::thread_rng().gen();
    if succeeds {
        log(addr, "succeeded");
        Ok(())
    } else {
        log(addr, "failed");
        Err(io::Error::new(
            io::ErrorKind::ConnectionAborted,
            "connection failed",
        ))
    }
}

async fn happy_eyeballs(addrs: impl IntoIterator<Item = SocketAddr>) -> io::Result<Connection> {
    let mut attempts = FuturesUnordered::new();
    let mut current_attempt_id = 0u64;

    // For each addr in the iterator, create a future to connect to it, and add
    // that future to the attempts collection. Then wait for any of the
    // following:
    //   1. any attempt to succeed
    //   2. the most recent attempt to fail
    //   3. the WAIT_MILLIS timeout to pass
    // In the first case, return the successful connection. In the other cases,
    // continue looping to the next addr.
    for addr in addrs {
        current_attempt_id += 1;
        attempts.push(async move {
            let result = dummy_connect(addr).await;
            (current_attempt_id, result)
        });
        let timeout = task::sleep(Duration::from_millis(WAIT_MILLIS)).fuse();
        pin_mut!(timeout);

        // Several attempts might fail while we wait for the one we just
        // spawned, so we select in a loop.
        loop {
            select! {
                _ = timeout => break,
                attempt = attempts.next() => {
                    let (id, result) = attempt.expect("at least one attempt in flight");
                    match result {
                        Ok(conn) => return Ok(conn),
                        Err(_) => {
                            // If the failure was from any job besides the
                            // current one, continue this wait loop.
                            if id == current_attempt_id {
                                break;
                            }
                        },
                    }
                }
            }
        }
    }

    // At this point we've spawned all the attempts we're going to spawn. Await
    // any remaining attempts that haven't already failed.
    while let Some((_, result)) = attempts.next().await {
        if let Ok(conn) = result {
            return Ok(conn);
        }
    }

    Err(io::Error::new(
        io::ErrorKind::ConnectionAborted,
        "all addresses failed to connect",
    ))
}

fn main() {
    // Initialize the START time first thing.
    let _ = *START;
    let addrs = vec![
        SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(127, 0, 0, 1), 80)),
        SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(127, 0, 0, 2), 80)),
        SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(127, 0, 0, 3), 80)),
    ];
    match task::block_on(happy_eyeballs(addrs)) {
        Ok(_) => eprintln!("Connected!"),
        Err(_) => eprintln!("Failed!"),
    }
}
