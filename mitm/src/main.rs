//! A tool for inspecting traffic between a Wesnoth client and the `wesnothd` server.
use ::tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use ::tokio::net::{TcpListener, TcpStream};


async fn run_middle(mut reader: OwnedReadHalf, mut writer: OwnedWriteHalf, mut on_msg: impl FnMut(&[u8])) {
    let mut buf = Vec::with_capacity(1024);
    loop {
        match reader.read_buf(&mut buf).await {
            Ok(n) if n != 0 => {
                on_msg(&buf);
                match writer.write_all(&buf).await {
                    Ok(()) => (),
                    Err(e) => ::tracing::warn!("write failure: {:?}", e),
                }
                buf.clear();
            },
            Ok(_) => {
                ::tracing::info!("connection ended");
                break
            },
            Err(e) => ::tracing::warn!("read failure: {:?}", e),
        }
    }
}

enum Side {
    Server,
    Client,
}
impl Side {
    fn name(&self) -> &'static str {
        match self {
            Side::Server => "server",
            Side::Client => "client",
        }
    }
}

fn process_msg(side: Side) -> impl FnMut(&[u8]) {
    use ::flate2::read::MultiGzDecoder;
    use ::std::io::Read;
    // Let's give each connection a megabyte of wiggle room.
    // This *should* be significantly more than it needs, but we'll see.
    // We use this to store up bytes from multiple `.read()` calls
    // and find the edges of messages as per Wesnoth's network protocol.
    // TODO: consider using a `VecDeque` because of how much we drain the front of the buffer
    let mut buf = Vec::<u8>::with_capacity(1024 * 1024);
    enum State {
        PreHandshake,
        Normal,
    }
    let mut state = State::PreHandshake;
    move |msg| {
        buf.extend(msg);
        match state {
            // The Wesnoth network protocol goes like this:
            // 1. Start TCP Session
            // 2. Perform Handshake
            // 3. Exchange data via gzipped WML
            //   - Messages are denoted by the ending of segments of length
            //     given by a big endian `u32` prefix.
            State::PreHandshake => {
                // consume initial handshake, consider pointing out incorrect handshakes
                match side {
                    Side::Server => {
                        if let [0, 0, 0, 42, ..] = *buf {
                            buf.drain(..4);
                            state = State::Normal;
                        } else if let [a, b, c, d, ..] = *buf {
                            ::tracing::warn!("incorrect server handshake [{}, {}, {}, {}]", a, b, c, d);
                            buf.drain(..4);
                            state = State::Normal;
                        }
                    },
                    Side::Client => {
                        if let [0, 0, 0, 0, ..] = *buf {
                            buf.drain(..4);
                            state = State::Normal;
                        } else if let [a, b, c, d, ..] = *buf {
                            ::tracing::warn!("incorrect client handshake [{}, {}, {}, {}]", a, b, c, d);
                            buf.drain(..4);
                            state = State::Normal;
                        }
                    },
                }
            },
            State::Normal => {
                // decompress gzipped payloads
                // Note, not necessarily all payloads will be gzipped.
                // We decode them on a best effort basis, based on observation
                // of patterns in payloads.
                while let [a, b, c, d, ref rest @ ..] = *buf {
                    let len = u32::from_be_bytes([a, b, c, d]) as usize;
                    if rest.len() >= len {
                        ::tracing::info!("{}: {:?}", side.name(), &rest[..len]);
                        let mut gz = MultiGzDecoder::new(&rest[..len]);
                        let mut gz_buf = Vec::new();
                        let _ = dbg!(gz.read_to_end(&mut gz_buf));
                        let s = String::from_utf8_lossy(&gz_buf);
                        ::tracing::info!("[decoded] {}: {:?}", side.name(), gz_buf);
                        ::tracing::info!("[decoded(utf-8)] {}: {:?}", side.name(), s);
                        // remove the now handled message from the buffer
                        buf.drain(..4 + len);
                    } else {
                        ::tracing::info!("waiting for more input...");
                        break
                    }
                }
            }
        };
    }
}


#[::tracing::instrument]
async fn start_session(client: TcpStream) -> Result<(), ()> {
    let (client_rx, client_tx) = client.into_split();
    let (server_rx, server_tx) = TcpStream::connect("127.0.0.1:15000").await.map_err(|_| ())?.into_split();
    ::tokio::spawn(run_middle(client_rx, server_tx, process_msg(Side::Client)));
    ::tokio::spawn(run_middle(server_rx, client_tx, process_msg(Side::Server)));
    Ok(())
}

#[::tokio::main]
async fn main() {
    // install global collector configured based on RUST_LOG env var.
    tracing_subscriber::fmt::init();
    let listener = TcpListener::bind("127.0.0.1:10900").await.unwrap();
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                ::tracing::info!("starting new session...");
                ::tokio::spawn(start_session(stream));
            },
            Err(e) => {
                ::tracing::warn!("failed to accept a connection: {:?}", e);
            },
        }
    }
}
