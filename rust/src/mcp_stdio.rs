//! The stdio transport `sobre mcp` serves on.
//!
//! The transport is rmcp's own newline-delimited JSON-RPC transport
//! ([`AsyncRwTransport`]): it skips a line that is not JSON and a notification
//! outside MCP without closing the session, and answers well-formed JSON that
//! is not a JSON-RPC message with an `Invalid Request` error.
//!
//! The one thing this module adds is a length cap on stdin. The transport
//! reads a line into memory until its newline, however long it gets; the
//! length limit rmcp offers (`JsonRpcMessageCodec::new_with_max_length`)
//! applies to the codec, which the transport does not read through. So stdin
//! goes through [`LineCap`], which fails the read once a line passes
//! [`MAX_MESSAGE_BYTES`]. The transport treats that as the end of the stream
//! and the session closes, so a host that never sends a newline cannot grow
//! the server's memory without bound.

use std::io;
use std::pin::Pin;
use std::task::{Context, Poll, ready};

use rmcp::RoleServer;
use rmcp::transport::async_rw::AsyncRwTransport;
use tokio::io::{AsyncRead, ReadBuf, Stdin, Stdout};

/// The longest MCP message the server reads: 32 MiB.
pub const MAX_MESSAGE_BYTES: usize = 32 * 1024 * 1024;

/// The server's transport over this process's stdin and stdout.
pub fn stdio_transport() -> AsyncRwTransport<RoleServer, LineCap<Stdin>, Stdout> {
    AsyncRwTransport::new_server(
        LineCap::new(tokio::io::stdin(), MAX_MESSAGE_BYTES),
        tokio::io::stdout(),
    )
}

/// A reader that fails with [`io::ErrorKind::InvalidData`] once a line runs
/// past `max` bytes without a newline.
pub struct LineCap<R> {
    inner: R,
    max: usize,
    line_len: usize,
}

impl<R> LineCap<R> {
    pub fn new(inner: R, max: usize) -> Self {
        Self {
            inner,
            max,
            line_len: 0,
        }
    }
}

impl<R: AsyncRead + Unpin> AsyncRead for LineCap<R> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let start = buf.filled().len();
        ready!(Pin::new(&mut this.inner).poll_read(cx, buf))?;
        let too_long = buf.filled()[start..].iter().any(|&byte| {
            this.line_len = if byte == b'\n' { 0 } else { this.line_len + 1 };
            this.line_len > this.max
        });
        if too_long {
            // A read that fails must not have filled anything.
            buf.set_filled(start);
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("MCP message longer than {} bytes", this.max),
            )));
        }
        Poll::Ready(Ok(()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;

    async fn read_all(input: &[u8], max: usize) -> io::Result<Vec<u8>> {
        let mut out = Vec::new();
        LineCap::new(input, max).read_to_end(&mut out).await?;
        Ok(out)
    }

    #[tokio::test]
    async fn lines_within_the_cap_pass_through_unchanged() {
        let input = b"12345678\n1234\n12345678";
        assert_eq!(read_all(input, 8).await.unwrap(), input);
    }

    #[tokio::test]
    async fn a_line_past_the_cap_fails_the_read() {
        let err = read_all(b"1234\n123456789\n", 8).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[tokio::test]
    async fn the_count_carries_across_reads_until_a_newline() {
        // Reads of 4 bytes each: the second line spans three of them.
        let (mut client, server) = tokio::io::duplex(4);
        let writer = tokio::spawn(async move {
            use tokio::io::AsyncWriteExt;
            let _ = client.write_all(b"ab\n0123456789\n").await;
        });
        let mut out = Vec::new();
        let err = LineCap::new(server, 8)
            .read_to_end(&mut out)
            .await
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        writer.await.unwrap();
    }
}
