//! Length-prefixed frame I/O (4-byte big-endian length, then JSON), matching the
//! machine service's local framing.

use std::io;

use serde_json::Value;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

fn invalid(why: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, why)
}

/// Reads one raw frame (prefix included). `Ok(None)` on a clean end of stream before
/// the first prefix byte. A zero or larger-than-`limit` length is an error.
pub async fn read_raw<R: AsyncRead + Unpin>(
    reader: &mut R,
    limit: usize,
) -> io::Result<Option<Vec<u8>>> {
    let mut prefix = [0u8; 4];
    let mut filled = 0;
    while filled < 4 {
        let n = reader.read(&mut prefix[filled..]).await?;
        if n == 0 {
            if filled == 0 {
                return Ok(None);
            }
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "incomplete_frame",
            ));
        }
        filled += n;
    }
    let size = u32::from_be_bytes(prefix) as usize;
    if size == 0 || size > limit {
        return Err(invalid("frame_too_large"));
    }
    let mut frame = vec![0u8; 4 + size];
    frame[..4].copy_from_slice(&prefix);
    reader.read_exact(&mut frame[4..]).await?;
    Ok(Some(frame))
}

/// Reads and decodes one JSON frame; `Ok(None)` on a clean end of stream.
pub async fn read_json<R: AsyncRead + Unpin>(
    reader: &mut R,
    limit: usize,
) -> io::Result<Option<Value>> {
    match read_raw(reader, limit).await? {
        None => Ok(None),
        Some(frame) => workspace_protocol::decode_frame(&frame)
            .map(Some)
            .map_err(|_| invalid("invalid_json")),
    }
}

/// Encodes and writes one JSON frame.
pub async fn write_json<W: AsyncWrite + Unpin>(writer: &mut W, value: &Value) -> io::Result<()> {
    let frame = workspace_protocol::encode_frame(value).map_err(|_| invalid("invalid_frame"))?;
    writer.write_all(&frame).await
}
