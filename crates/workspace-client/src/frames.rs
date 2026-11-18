//! The machine service's frames: a 4-byte big-endian length, then that many bytes of JSON,
//! bounded by [`workspace_protocol::FRAME_BYTES`].
use serde_json::Value;
use std::io;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

fn failure(code: &'static str) -> io::Error {
    io::Error::other(code)
}

/// Reads one frame. An empty or oversized frame is `frame_too_large`, bad JSON `invalid_json`.
pub async fn read_frame<R: AsyncRead + Unpin>(reader: &mut R) -> io::Result<Value> {
    let n = reader.read_u32().await? as usize;
    if n == 0 || n > workspace_protocol::FRAME_BYTES {
        return Err(failure("frame_too_large"));
    }
    let mut frame = Vec::with_capacity(4 + n);
    frame.extend((n as u32).to_be_bytes());
    frame.resize(4 + n, 0);
    reader.read_exact(&mut frame[4..]).await?;
    workspace_protocol::decode_frame(&frame).map_err(|_| failure("invalid_json"))
}

/// Writes one frame and flushes it (a QUIC stream sends what is flushed).
pub async fn write_frame<W: AsyncWrite + Unpin>(writer: &mut W, value: &Value) -> io::Result<()> {
    let frame = workspace_protocol::encode_frame(value).map_err(|_| failure("invalid_frame"))?;
    writer.write_all(&frame).await?;
    writer.flush().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn frames_round_trip_and_bounds_are_refused() {
        let (mut a, mut b) = tokio::io::duplex(1 << 16);
        write_frame(&mut a, &json!({"jsonrpc":"2.0","id":"1"}))
            .await
            .unwrap();
        assert_eq!(read_frame(&mut b).await.unwrap()["id"], "1");
        a.write_all(&0u32.to_be_bytes()).await.unwrap();
        assert_eq!(
            read_frame(&mut b).await.unwrap_err().to_string(),
            "frame_too_large"
        );
        a.write_all(&((workspace_protocol::FRAME_BYTES as u32) + 1).to_be_bytes())
            .await
            .unwrap();
        assert_eq!(
            read_frame(&mut b).await.unwrap_err().to_string(),
            "frame_too_large"
        );
    }
}
