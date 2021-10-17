//! A Wesnoth server for playing over email. Not close to ready for use.
mod wml;

use ::core::cmp::Ordering;
use ::tokio::io::{AsyncReadExt, AsyncWriteExt};
use ::tokio::net::{TcpListener, TcpStream};
use ::once_cell::sync::Lazy;

struct Disconnect;

/// Send an error to the client.
/// If we don't send one of these, the client will hang for quite
/// a while before giving up if we disconnect.
async fn send_error(stream: &mut TcpStream, error: &str, bzip2: bool) -> Result<(), Disconnect> {
    use ::flate2::Compression;
    use ::flate2::write::GzEncoder;
    use ::std::io::Write;
    let msg = format!("[error]message=\"{}\"[/error]", error);
    let mut gzip = GzEncoder::new(Vec::new(), Compression::best());
    gzip.write_all(msg.as_bytes()).unwrap();
    stream.write_all(&gzip.finish().unwrap()).await.map_err(|_| Disconnect)?;
    Ok(())
}

enum SessionError {
    Disconnect,
    IncorrectHandshake,
}
impl From<Disconnect> for SessionError {
    fn from(Disconnect: Disconnect) -> Self {
        Self::Disconnect
    }
}

/// `wesnothd` uses `htonl(42)` into a union with `char buf[4]` to accomplish this.
///
/// Summary of `htonl` from manual page:
/// > The  htonl()  function converts the unsigned integer hostlong from
/// > host byte order to network byte order.
///
/// "Network byte order" is big endian.
const HANDSHAKE_RESPONSE: [u8; 4] = u32::to_be_bytes(42);

/// Sent to clients to query their version.
static VERSION_QUERY_RESPONSE: Lazy<&[u8]> = Lazy::new(|| {
    use ::flate2::Compression;
    use ::flate2::write::GzEncoder;
    use ::std::io::Write;
    let mut gzip = GzEncoder::new(Vec::new(), Compression::best());
    gzip.write_all("[version]\n[/version]\n".as_bytes()).unwrap();
    // Since this will never be freed anyway, we can just leak it.
    Vec::leak(gzip.finish().unwrap())
});


#[::tracing::instrument]
async fn handle_connection(mut stream: TcpStream) -> Result<(), SessionError> {
    use SessionError::*;
    dbg!(&stream);
    // TODO: Perform handshake
    let mut buf = [0; 4];
    stream.read(&mut buf).await.map_err(|_| Disconnect)?;
    match buf.cmp(&[0; 4]) {
        Ordering::Equal => (),
        _ => {
            ::tracing::error!("incorrect handshake");
            return Err(SessionError::IncorrectHandshake)
        },
    }
    stream.write(&HANDSHAKE_RESPONSE).await.map_err(|_| Disconnect)?;
    // Right here, prior to asking for the client's version, is where `wesnothd`
    // boots people from connecting if they're banned or the connection
    // limit has been reached.
    stream.write_all(&VERSION_QUERY_RESPONSE).await.map_err(|_| Disconnect)?;

    let mut buf = [0; 4];
    println!("Waiting for version query response...");
    stream.read(&mut buf).await.map_err(|_| Disconnect)?;
    dbg!(&buf);

    // This loop just prevents us from closing the stream,
    // so we can see if Wesnoth will keep it open.
    loop {
        // Permit 
        ::tokio::task::yield_now().await;
    }
}

#[::tokio::main]
async fn main() {
    // install global collector configured based on RUST_LOG env var.
    tracing_subscriber::fmt::init();
    println!("{:?}", &*VERSION_QUERY_RESPONSE);
    use ::std::io::Read;
    let mut gz = ::flate2::read::GzDecoder::new(&**VERSION_QUERY_RESPONSE);
    let mut buf = Vec::new();
    gz.read_to_end(&mut buf);
    println!("{:?}", buf);
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
