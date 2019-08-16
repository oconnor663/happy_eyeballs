#![feature(async_await)]

use tokio;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;

use std::error::Error;
use std::net::ToSocketAddrs;

#[tokio::main]
pub async fn main() -> Result<(), Box<dyn Error>> {
    for addr in "google.com:80".to_socket_addrs()? {
        dbg!(addr);
    }

    let addr = "127.0.0.1:6142".parse()?;

    // Open a TCP stream to the socket address.
    //
    // Note that this is the Tokio TcpStream, which is fully async.
    let mut stream = TcpStream::connect(&addr).await?;
    println!("created stream");

    let result = stream.write(b"hello world\n").await;
    println!("wrote to stream; success={:?}", result.is_ok());

    Ok(())
}
