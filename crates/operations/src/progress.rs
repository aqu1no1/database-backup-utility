use std::pin::Pin;
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, ReadBuf};

const REPORT_EVERY_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    ConnectionValidated,
    Dump,
    Encryption,
    Checksum,
    Published,
}

impl Stage {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ConnectionValidated => "connection_validated",
            Self::Dump => "dump",
            Self::Encryption => "encryption",
            Self::Checksum => "checksum",
            Self::Published => "published",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Progress {
    Stage(Stage),
    BytesWritten(u64),
}

pub trait ProgressSink: Send + Sync {
    fn report(&self, progress: Progress);
}

impl<F: Fn(Progress) + Send + Sync> ProgressSink for F {
    fn report(&self, progress: Progress) {
        self(progress);
    }
}

pub(crate) struct CountingReader<'a, R> {
    inner: R,
    sink: &'a dyn ProgressSink,
    total: u64,
    reported: u64,
}

impl<'a, R> CountingReader<'a, R> {
    pub(crate) fn new(inner: R, sink: &'a dyn ProgressSink) -> Self {
        Self {
            inner,
            sink,
            total: 0,
            reported: 0,
        }
    }
}

impl<R: AsyncRead + Unpin> AsyncRead for CountingReader<'_, R> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let before = buf.filled().len();
        let poll = Pin::new(&mut self.inner).poll_read(cx, buf);
        if matches!(poll, Poll::Ready(Ok(()))) {
            let read = (buf.filled().len() - before) as u64;
            self.total += read;
            let pending = self.total - self.reported;
            if pending >= REPORT_EVERY_BYTES || (read == 0 && pending > 0) {
                self.reported = self.total;
                self.sink.report(Progress::BytesWritten(self.total));
            }
        }
        poll
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use tokio::io::AsyncReadExt;

    use super::{CountingReader, Progress, REPORT_EVERY_BYTES};

    #[tokio::test]
    async fn reports_increasing_totals_and_the_final_size() {
        let size = REPORT_EVERY_BYTES * 2 + 10;
        let content = vec![7_u8; size as usize];
        let reports = Mutex::new(Vec::new());
        let sink = |progress: Progress| reports.lock().unwrap().push(progress);
        let mut reader = CountingReader::new(content.as_slice(), &sink);
        let mut copied = Vec::new();

        reader.read_to_end(&mut copied).await.unwrap();

        let totals: Vec<u64> = reports
            .lock()
            .unwrap()
            .iter()
            .filter_map(|progress| match progress {
                Progress::BytesWritten(total) => Some(*total),
                Progress::Stage(_) => None,
            })
            .collect();
        assert_eq!(copied, content);
        assert!(totals.is_sorted());
        assert_eq!(totals.last(), Some(&size));
    }
}
