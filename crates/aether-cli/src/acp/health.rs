//! The plain-HTTP health probe the deploy contract checks.
//!
//! The runner's `[[health]]` check asks the running container for `/healthz`
//! and expects an ordinary HTTP answer, which the WebSocket handshake cannot
//! produce: tungstenite answers a failed handshake with an error status, never
//! with `200`. The request head is therefore read before the handshake starts
//! -- a probe is answered here, and every other request is replayed to the
//! handshake byte for byte.

use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::net::TcpStream;

/// The path the deploy contract's health check probes.
pub(crate) const HEALTH_PATH: &str = "/healthz";

/// The largest request head read before the server decides what it is.
const MAX_HEAD: usize = 64 * 1024;

/// The answer to a health probe: `200 OK` with the body `ok`.
const HEALTH_RESPONSE: &[u8] =
    b"HTTP/1.1 200 OK\r\ncontent-type: text/plain; charset=utf-8\r\ncontent-length: 2\r\nconnection: close\r\n\r\nok";

/// A socket whose reads start with the bytes already taken from it.
pub(crate) struct PrefixedStream {
    socket: TcpStream,
    prefix: Vec<u8>,
    offset: usize,
}

/// Read the request head from `socket`.
///
/// The head ends at the first blank line; a head that never ends is cut off
/// at [`MAX_HEAD`], so a connection cannot keep the server reading forever.
/// Bytes that arrived with the head are kept, so that the handshake sees the
/// connection in the order the client wrote it.
pub(crate) async fn read_head(socket: &mut TcpStream) -> io::Result<Vec<u8>> {
    let mut head = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    loop {
        if head_end(&head).is_some() {
            return Ok(head);
        }
        if head.len() >= MAX_HEAD {
            return Ok(head);
        }
        let read = socket.read(&mut chunk).await?;
        if read == 0 {
            return Ok(head);
        }
        head.extend_from_slice(&chunk[..read]);
    }
}

/// Whether the head is a `GET /healthz` request.
pub(crate) fn is_health_request(head: &[u8]) -> bool {
    const METHOD: &[u8] = b"GET ";
    head.starts_with(METHOD)
        && head[METHOD.len()..].starts_with(HEALTH_PATH.as_bytes())
        && head.get(METHOD.len() + HEALTH_PATH.len()) == Some(&b' ')
}

/// Answer a health probe on `socket`.
pub(crate) async fn write_health_response(socket: &mut TcpStream) -> io::Result<()> {
    socket.write_all(HEALTH_RESPONSE).await?;
    socket.flush().await?;
    let _ = socket.shutdown().await;
    Ok(())
}

impl PrefixedStream {
    /// Replay `prefix` before the bytes `socket` still holds.
    pub(crate) fn new(socket: TcpStream, prefix: Vec<u8>) -> Self {
        Self { socket, prefix, offset: 0 }
    }
}

impl AsyncRead for PrefixedStream {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buffer: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if this.offset < this.prefix.len() {
            let take = (this.prefix.len() - this.offset).min(buffer.remaining());
            buffer.put_slice(&this.prefix[this.offset..this.offset + take]);
            this.offset += take;
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut this.socket).poll_read(cx, buffer)
    }
}

impl AsyncWrite for PrefixedStream {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buffer: &[u8]) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().socket).poll_write(cx, buffer)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().socket).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().socket).poll_shutdown(cx)
    }
}

/// The end of the request head, just past its blank line.
fn head_end(head: &[u8]) -> Option<usize> {
    head.windows(4).position(|window| window == b"\r\n\r\n").map(|start| start + 4)
}
