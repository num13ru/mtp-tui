//! Streaming download/upload support.

use crate::ptp::ReceiveStream;
use crate::Error;
use bytes::Bytes;
use std::ops::ControlFlow;

/// Progress information for transfers.
#[derive(Debug, Clone)]
pub struct Progress {
    /// Bytes transferred so far.
    pub bytes_transferred: u64,
    /// Total bytes (if known).
    pub total_bytes: Option<u64>,
}

impl Progress {
    /// Progress as a percentage (0.0 to 100.0).
    #[must_use]
    pub fn percent(&self) -> f64 {
        self.fraction() * 100.0
    }

    /// Progress as a fraction (0.0 to 1.0).
    #[must_use]
    pub fn fraction(&self) -> f64 {
        self.total_bytes.map_or(1.0, |total| {
            if total == 0 {
                1.0
            } else {
                self.bytes_transferred as f64 / total as f64
            }
        })
    }
}

/// A file download in progress with true USB streaming.
///
/// This struct wraps the low-level `ReceiveStream` and provides convenient
/// methods for tracking progress. Data is streamed directly from USB as
/// chunks arrive, without buffering the entire file in memory.
///
/// # Important
///
/// The MTP session is locked while this download is active. You must consume
/// the entire download (or drop it) before calling other storage methods.
///
/// # Example
///
/// ```rust,ignore
/// let mut download = storage.download_stream(handle).await?;
/// println!("Downloading {} bytes...", download.size());
///
/// while let Some(chunk) = download.next_chunk().await {
///     let bytes = chunk?;
///     file.write_all(&bytes).await?;
///     println!("Progress: {:.1}%", download.progress() * 100.0);
/// }
/// ```
pub struct FileDownload {
    size: u64,
    bytes_received: u64,
    stream: ReceiveStream,
}

impl FileDownload {
    /// Create a new FileDownload wrapping a ReceiveStream.
    pub(crate) fn new(size: u64, stream: ReceiveStream) -> Self {
        Self {
            size,
            bytes_received: 0,
            stream,
        }
    }

    /// Total file size in bytes.
    #[must_use]
    pub fn size(&self) -> u64 {
        self.size
    }

    /// Bytes received so far.
    #[must_use]
    pub fn bytes_received(&self) -> u64 {
        self.bytes_received
    }

    /// Progress as a fraction (0.0 to 1.0).
    #[must_use]
    pub fn progress(&self) -> f64 {
        if self.size == 0 {
            1.0
        } else {
            self.bytes_received as f64 / self.size as f64
        }
    }

    /// Get the next chunk of data from USB.
    ///
    /// Returns `None` when the download is complete.
    pub async fn next_chunk(&mut self) -> Option<Result<Bytes, Error>> {
        match self.stream.next_chunk().await {
            Some(Ok(bytes)) => {
                self.bytes_received += bytes.len() as u64;
                Some(Ok(bytes))
            }
            Some(Err(e)) => Some(Err(e)),
            None => None,
        }
    }

    /// Consume the download and iterate with a progress callback.
    ///
    /// Calls `on_progress` after each chunk. Return `ControlFlow::Break(())`
    /// to cancel the download.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let data = download.collect_with_progress(|progress| {
    ///     println!("{:.1}%", progress.percent().unwrap_or(0.0));
    ///     ControlFlow::Continue(())
    /// }).await?;
    /// ```
    pub async fn collect_with_progress<F>(mut self, mut on_progress: F) -> Result<Vec<u8>, Error>
    where
        F: FnMut(Progress) -> ControlFlow<()>,
    {
        let mut data = Vec::with_capacity(self.size as usize);

        while let Some(result) = self.next_chunk().await {
            let chunk = result?;
            data.extend_from_slice(&chunk);

            let progress = Progress {
                bytes_transferred: self.bytes_received,
                total_bytes: Some(self.size),
            };

            if let ControlFlow::Break(()) = on_progress(progress) {
                return Err(Error::Cancelled);
            }
        }

        Ok(data)
    }

    /// Collect all remaining data into a `Vec<u8>`.
    ///
    /// This consumes the download and buffers all data in memory.
    pub async fn collect(self) -> Result<Vec<u8>, Error> {
        self.stream.collect().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_calculations() {
        let cases = [
            (50, Some(100), 50.0, 0.5),
            (100, Some(100), 100.0, 1.0),
            (25, Some(100), 25.0, 0.25),
            (0, Some(0), 100.0, 1.0), // Empty file
            (50, None, 100.0, 1.0),   // Unknown total defaults to complete
        ];
        for (transferred, total, expected_pct, expected_frac) in cases {
            let p = Progress {
                bytes_transferred: transferred,
                total_bytes: total,
            };
            assert_eq!(
                p.percent(),
                expected_pct,
                "percent failed for {transferred}/{total:?}"
            );
            assert_eq!(
                p.fraction(),
                expected_frac,
                "fraction failed for {transferred}/{total:?}"
            );
        }

        // Large numbers
        let large = Progress {
            bytes_transferred: u64::MAX / 2,
            total_bytes: Some(u64::MAX),
        };
        let frac = large.fraction();
        assert!(frac > 0.49 && frac < 0.51);
    }
}
