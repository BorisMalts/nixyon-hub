use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt};

use crate::protocol::{MAX_LINE_BYTES, ServerFrame, WireError, encode};

#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Wire(#[from] WireError),
}

pub async fn read_line<R: AsyncBufRead + Unpin>(
    reader: &mut R,
) -> Result<Option<Vec<u8>>, FrameError> {
    read_line_max(reader, MAX_LINE_BYTES).await
}

pub async fn read_line_max<R: AsyncBufRead + Unpin>(
    reader: &mut R,
    max: usize,
) -> Result<Option<Vec<u8>>, FrameError> {
    let mut line = Vec::new();
    loop {
        let buf = reader.fill_buf().await?;
        if buf.is_empty() {
            return Ok(if line.is_empty() { None } else { Some(line) });
        }
        let (chunk, found) = match buf.iter().position(|b| *b == b'\n') {
            Some(i) => (&buf[..i], true),
            None => (buf, false),
        };
        if line.len() + chunk.len() > max {
            return Err(WireError::TooLong.into());
        }
        line.extend_from_slice(chunk);
        let consumed = chunk.len() + usize::from(found);
        reader.consume(consumed);
        if found {
            return Ok(Some(line));
        }
    }
}

pub async fn send<W: AsyncWrite + Unpin>(
    writer: &mut W,
    frame: &ServerFrame,
) -> Result<(), FrameError> {
    writer.write_all(&encode(frame)?).await?;
    writer.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::BufReader;

    #[tokio::test]
    async fn lines_are_split_and_oversized_line_is_refused() {
        let mut reader = BufReader::with_capacity(4, &b"one\ntwo\nthree"[..]);
        assert_eq!(read_line(&mut reader).await.unwrap().unwrap(), b"one");
        assert_eq!(read_line(&mut reader).await.unwrap().unwrap(), b"two");
        assert_eq!(read_line(&mut reader).await.unwrap().unwrap(), b"three");
        assert!(read_line(&mut reader).await.unwrap().is_none());

        let huge = vec![b'x'; MAX_LINE_BYTES + 10];
        let mut reader = BufReader::new(&huge[..]);
        assert!(matches!(
            read_line(&mut reader).await,
            Err(FrameError::Wire(WireError::TooLong))
        ));
    }
}
