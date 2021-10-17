//! A Wesnoth server for playing over email. Not close to ready for use.
mod wml;
mod stream;

use ::core::cmp::Ordering;
use ::tokio::io::{AsyncReadExt, AsyncWriteExt};
use ::tokio::net::{TcpListener, TcpStream};

#[::tracing::instrument]
async fn handle_connection(s: TcpStream) -> Result<(), ()> {
    let (mut reader, mut writer) = stream::server_handshake(s).await?;
    writer.write(b"[version]\n[/version]\n").await.map_err(|e| ::tracing::debug!("write failure: {:?}", e))?;
    writer.write(b"[error]message=\"ur banned d00d\"\n[/error]").await.map_err(|e| ::tracing::debug!("write failure: {:?}", e))?;
    loop {
        ::tokio::task::yield_now().await
    }
}

#[::tokio::main]
async fn main() {
    // install global collector configured based on RUST_LOG env var.
    tracing_subscriber::fmt::init();
    let listener = TcpListener::bind("127.0.0.1:15000").await.unwrap();
    loop {
        match listener.accept().await {
            Ok((socket, _)) => {
                ::tokio::spawn(handle_connection(socket));
            },
            Err(e) => {
                ::tracing::warn!("failed to accept a connection: {:?}", e);
            },
        }
    }
}
