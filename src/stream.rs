//! A protocol wrapper for Wesnoth message streams on top of TCP.
use ::tokio::io::{AsyncReadExt, AsyncWriteExt};
use ::tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use ::tokio::net::TcpStream;

// TODO: have option for doing bzip2 based compression instead of gzip compression

// TODO: decide how to do allocation limits for the Reader buffer,
// as Wesnoth clients are, in the general case, run by untrusted users
// (although `wesmaild` is, at least initially, meant to be run locally with the client)
pub struct Reader {
    half: OwnedReadHalf,
    // TODO: strongly consider using VecDeque for this,
    // since we constantly drain from the front
    buf: Vec<u8>,
}
impl Reader {
    fn from_raw(half: OwnedReadHalf, buf: Vec<u8>) -> Self { Self { half, buf } }
    // TODO: implement either the AsyncRead or Stream trait,
    // or even do WML parsing without copying out the buffer,
    // in which case definitely implement the Stream trait
    pub async fn read(&mut self) -> Result<Box<[u8]>, ()> {
        loop {
            if let [a, b, c, d, ref rest @ ..] = *self.buf {
                let len = u32::from_be_bytes([a, b, c, d]) as usize;
                if rest.len() >= len {
                    // decompress message and return
                    use ::flate2::read::MultiGzDecoder;
                    use ::std::io::Read;
                    let mut gz = MultiGzDecoder::new(&rest[..len]);
                    let mut gz_buf = Vec::new();
                    match gz.read_to_end(&mut gz_buf) {
                        Ok(_n) => (),
                        Err(e) => {
                            ::tracing::debug!("decompression failed: {:?}", e);
                            return Err(())
                        },
                    }
                    return Ok(gz_buf.into_boxed_slice())
                }
            }
            match self.half.read_buf(&mut self.buf).await {
                Ok(n) if n != 0 => (),
                Ok(_) => {
                    // since we currently assume that `self.buf` will
                    // never run out of space, this means that
                    // the stream is no longer able to produce bytes
                    return Err(())
                },
                Err(e) => {
                    ::tracing::debug!("read failure: {:?}", e);
                    return Err(())
                }
            }
        }
    }
}

pub struct Writer {
    half: OwnedWriteHalf,
}
impl Writer {
    fn from_raw(half: OwnedWriteHalf) -> Self { Self { half } }
}

/// `wesnothd` uses `htonl(42)` into a union with `char buf[4]` to accomplish this.
///
/// Summary of `htonl` from manual page:
/// > The  htonl()  function converts the unsigned integer hostlong from
/// > host byte order to network byte order.
///
/// "Network byte order" is big endian.
const SERVER_HANDSHAKE_RESPONSE: [u8; 4] = u32::to_be_bytes(42);


// Note that the opening handshake appears to be the only difference in protocol,
// at this level, between the Wesnoth client and server.
/// Perform the necessary handshake, as the server, to go from raw TCP to
/// distinct, compressed, blobs of WML.
pub async fn server_handshake(mut stream: TcpStream) -> Result<(Reader, Writer), ()> {
    let mut buf = Vec::<u8>::with_capacity(1024);
    loop {
        match stream.read_buf(&mut buf).await {
            Ok(n) if n != 0 => {
                if let [0, 0, 0, 0, ..] = *buf {
                    // correct client handshake, consume and send server handshake
                    match stream.write_all(&SERVER_HANDSHAKE_RESPONSE).await {
                        Ok(()) => (),
                        Err(e) => {
                            ::tracing::debug!("failed to send server handshake: {:?}", e);
                            return Err(())
                        }
                    }
                    buf.drain(..4);
                    // now return the Reader and Writer, which are ready to do
                    // compressed WML messages
                    let (reader, writer) = stream.into_split();
                    return Ok((Reader::from_raw(reader, buf), Writer::from_raw(writer)))
                } else if let [a, b, c, d, ..] = *buf {
                    ::tracing::debug!("incorrect client handshake [{}, {}, {}, {}]", a, b, c, d);
                    // TODO: consider having an option to tolerate incorrect handshakes
                    return Err(())
                }
            },
            Ok(_) => {
                // since we currently assume `buf` won't run out of space,
                // this means the stream is unable to produce bytes
                ::tracing::debug!("connection ended");
                return Err(())
            },
            Err(e) => {
                ::tracing::debug!("read failure: {:?}", e);
                return Err(())
            },
        }
    }
}

// TODO: consider providing client_handshake
