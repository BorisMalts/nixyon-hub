use std::pin::Pin;
use std::task::{Context, Poll};

use axum::body::{Bytes, HttpBody};
use http_body::{Frame, SizeHint};
use tokio::io::{AsyncRead, ReadBuf};
use tokio::sync::OwnedSemaphorePermit;

const CHUNK: u64 = 64 * 1024;

pub struct FileBody {
    file: tokio::fs::File,
    remaining: u64,
    // Места под отдачу держатся, пока тело не отдано или не брошено.
    _slots: Vec<OwnedSemaphorePermit>,
}

impl FileBody {
    pub fn new(
        file: std::fs::File,
        len: u64,
        slots: impl IntoIterator<Item = OwnedSemaphorePermit>,
    ) -> Self {
        Self {
            file: tokio::fs::File::from_std(file),
            remaining: len,
            _slots: slots.into_iter().collect(),
        }
    }
}

impl HttpBody for FileBody {
    type Data = Bytes;
    type Error = std::io::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, Self::Error>>> {
        let this = self.get_mut();
        if this.remaining == 0 {
            return Poll::Ready(None);
        }
        let mut buf = vec![0u8; usize::try_from(this.remaining.min(CHUNK)).unwrap_or(0)];
        let mut read = ReadBuf::new(&mut buf);
        match Pin::new(&mut this.file).poll_read(cx, &mut read) {
            Poll::Ready(Ok(())) => {
                let n = read.filled().len();
                if n == 0 {
                    return Poll::Ready(Some(Err(std::io::ErrorKind::UnexpectedEof.into())));
                }
                buf.truncate(n);
                this.remaining -= n as u64;
                Poll::Ready(Some(Ok(Frame::data(Bytes::from(buf)))))
            }
            Poll::Ready(Err(e)) => Poll::Ready(Some(Err(e))),
            Poll::Pending => Poll::Pending,
        }
    }

    fn is_end_stream(&self) -> bool {
        self.remaining == 0
    }

    fn size_hint(&self) -> SizeHint {
        SizeHint::with_exact(self.remaining)
    }
}
