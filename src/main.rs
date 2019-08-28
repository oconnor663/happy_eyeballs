use async_std::io;
use async_std::task;
use futures::channel::{mpsc, oneshot};
use futures::future::FusedFuture;
use futures::pin_mut;
use futures::prelude::*;
use futures::select;
use lazy_static::lazy_static;
use rand::prelude::*;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::time::{Duration, Instant};

const WAIT_MILLIS: u64 = 250;

lazy_static! {
    static ref START: Instant = Instant::now();
}

struct Canceler {
    drop_sender: Option<oneshot::Sender<()>>,
    cleanup_handle: task::JoinHandle<()>,
}

impl Canceler {
    fn new() -> (Canceler, CancelSpawner) {
        let (task_sender, mut task_receiver) = mpsc::unbounded();
        let spawner = CancelSpawner { task_sender };
        let (drop_sender, drop_receiver) = oneshot::channel();
        let cleanup_handle = task::spawn(async move {
            let mut handles = Vec::new();
            let mut cancel_senders = Vec::new();
            let mut fused_drop_receiver = drop_receiver.fuse();
            loop {
                select! {
                    pair = task_receiver.next() => {
                        if let Some((handle, cancel_sender)) = pair {
                            handles.push(handle);
                            cancel_senders.push(cancel_sender);
                        } else {
                            break;
                        }
                    }
                    _ = fused_drop_receiver => {
                        break;
                    }
                }
            }
            if !fused_drop_receiver.is_terminated() {
                let _ = fused_drop_receiver.await;
            }
            for cancel_sender in cancel_senders {
                let _ = cancel_sender.send(());
            }
            for handle in handles {
                handle.await;
            }
        });
        (
            Canceler {
                drop_sender: Some(drop_sender),
                cleanup_handle,
            },
            spawner,
        )
    }
}

impl Drop for Canceler {
    fn drop(&mut self) {
        self.drop_sender.take().unwrap().send(()).expect("Canceller::drop");
        task::block_on(&mut self.cleanup_handle);
    }
}

#[derive(Clone)]
struct CancelSpawner {
    task_sender: mpsc::UnboundedSender<(task::JoinHandle<()>, oneshot::Sender<()>)>,
}

impl CancelSpawner {
    fn spawn<F>(&mut self, future: F)
    where
        F: Future + Send + 'static,
    {
        // If the receiver is already gone, then cancellation has already
        // happened, and we shouldn't spawn this task. This check is racy,
        // however, so we also have to handle send failure below.
        if self.task_sender.is_closed() {
            return;
        }
        let (cancel_sender, cancel_receiver) = oneshot::channel::<()>();
        let handle = task::spawn(async move {
            pin_mut!(future);
            select! {
                _ = future.fuse() => {}
                _ = cancel_receiver.fuse() => {}
            }
        });
        let send_result = self.task_sender.unbounded_send((handle, cancel_sender));
        // Even though we checked to see whether the channel was closed at the
        // top, it might've closed in the meantime, and sending to it might
        // fail here. In this case, cancel the task immediately.
        if let Err(e) = send_result {
            let (handle, cancel_sender) = e.into_inner();
            let _ = cancel_sender.send(());
            task::block_on(handle);
        }
    }
}

fn log(addr: std::net::SocketAddr, verb: &str) {
    let millis = (Instant::now() - *START).as_millis();
    let last_digit = match addr.ip() {
        std::net::IpAddr::V4(ip) => *ip.octets().last().unwrap(),
        std::net::IpAddr::V6(ip) => *ip.octets().last().unwrap(),
    };
    eprintln!("{} {} at {} ms", last_digit, verb, millis);
}

type Connection = ();

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

fn attempt(
    mut addrs: impl Iterator<Item = SocketAddr> + Send + 'static,
    mut spawner: CancelSpawner,
    prev_failed: Option<oneshot::Receiver<()>>,
    mut success: mpsc::Sender<Connection>,
) -> impl Future<Output = ()> + Send + 'static {
    async move {
        let addr = if let Some(addr) = addrs.next() {
            addr
        } else {
            return;
        };
        if let Some(prev_failed) = prev_failed {
            let timeout = task::sleep(Duration::from_millis(WAIT_MILLIS));
            pin_mut!(timeout);
            select! {
                _ = timeout.fuse() => {}
                result = prev_failed.fuse() => {
                    // Short-circuit if the failure sender was dropped, because
                    // that means the previous attempt succeeded.
                    if let Err(oneshot::Canceled) = result {
                        return;
                    }
                }
            }
        }
        let (fail_sender, fail_receiver) = oneshot::channel();
        spawner.spawn(attempt(
            addrs,
            spawner.clone(),
            Some(fail_receiver),
            success.clone(),
        ));
        match dummy_connect(addr).await {
            Ok(conn) => {
                let _ = success.send(conn).await;
            }
            Err(_) => {
                let _ = fail_sender.send(());
            }
        }
    }
}

async fn happy_eyeballs<A, I>(addrs: A) -> io::Result<Connection>
where
    A: IntoIterator<Item = SocketAddr, IntoIter = I>,
    I: Iterator<Item = SocketAddr> + Send + 'static,
{
    let (_canceler, mut spawner) = Canceler::new();
    let (success_sender, mut success_receiver) = mpsc::channel(0);
    spawner.spawn(attempt(
        addrs.into_iter(),
        spawner.clone(),
        None,
        success_sender,
    ));

    if let Some(conn) = success_receiver.next().await {
        Ok(conn)
    } else {
        Err(io::Error::new(
            io::ErrorKind::ConnectionAborted,
            "all addresses failed to connect",
        ))
    }

    // _canceler implicitly dropped
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
