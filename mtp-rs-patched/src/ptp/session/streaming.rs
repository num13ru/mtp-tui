//! Streaming transfer operations.
//!
//! This module contains the `ReceiveStream` struct and methods for streaming
//! data transfers, allowing memory-efficient downloads and uploads without
//! buffering entire files in memory.

use crate::ptp::{
    container_type, pack_u16, pack_u32, unpack_u32, CommandContainer, ContainerType, ObjectHandle,
    OperationCode, ResponseCode, ResponseContainer,
};
use crate::transport::Transport;
use crate::Error;
use bytes::Bytes;
use futures::lock::OwnedMutexGuard;
use futures::Stream;
use std::sync::Arc;

use super::{PtpSession, HEADER_SIZE};

impl PtpSession {
    // =========================================================================
    // Streaming operations
    // =========================================================================

    /// Execute operation with streaming data receive.
    ///
    /// Returns a Stream that yields data chunks as they arrive from USB.
    /// The stream yields `Bytes` chunks (typically up to 64KB each).
    ///
    /// # Important
    ///
    /// The caller must consume the entire stream before calling any other
    /// session methods. The MTP session is locked while the stream is active.
    ///
    /// # Arguments
    ///
    /// * `operation` - The operation code to execute
    /// * `params` - Operation parameters
    ///
    /// # Returns
    ///
    /// A `ReceiveStream` that yields `Result<Bytes, Error>` chunks.
    pub async fn execute_with_receive_stream(
        self: &Arc<Self>,
        operation: OperationCode,
        params: &[u32],
    ) -> Result<ReceiveStream, Error> {
        // Clone the Arc for the lock
        let lock = Arc::clone(&self.operation_lock);
        let guard = lock.lock_owned().await;

        let tx_id = self.next_transaction_id();

        // Send command
        let cmd = CommandContainer {
            code: operation,
            transaction_id: tx_id,
            params: params.to_vec(),
        };
        self.transport.send_bulk(&cmd.to_bytes()).await?;

        Ok(ReceiveStream {
            transport: Arc::clone(&self.transport),
            _guard: guard,
            transaction_id: tx_id,
            operation,
            buffer: Vec::new(),
            container_length: 0,
            payload_yielded: 0,
            header_parsed: false,
            done: false,
        })
    }

    /// Execute operation with streaming data send.
    ///
    /// Accepts a Stream of data chunks to send. The total_size must be
    /// known upfront (MTP protocol requirement).
    ///
    /// # Arguments
    ///
    /// * `operation` - The operation code
    /// * `params` - Operation parameters
    /// * `total_size` - Total bytes that will be sent (REQUIRED by MTP protocol)
    /// * `data` - Stream of data chunks to send
    ///
    /// # Important
    ///
    /// The `total_size` must match the actual total bytes in the stream.
    /// MTP requires knowing the size before transfer begins.
    pub async fn execute_with_send_stream<S>(
        &self,
        operation: OperationCode,
        params: &[u32],
        total_size: u64,
        mut data: S,
    ) -> Result<ResponseContainer, Error>
    where
        S: Stream<Item = Result<Bytes, std::io::Error>> + Unpin,
    {
        use futures::StreamExt;

        let _guard = self.operation_lock.lock().await;
        let tx_id = self.next_transaction_id();

        // Send command
        let cmd = CommandContainer {
            code: operation,
            transaction_id: tx_id,
            params: params.to_vec(),
        };
        self.transport.send_bulk(&cmd.to_bytes()).await?;

        // Build complete data container (header + all payload)
        // MTP devices expect the entire data container in a single USB transfer
        let container_length = HEADER_SIZE as u64 + total_size;
        let mut buffer = Vec::with_capacity(container_length as usize);

        // Add header
        if container_length <= u32::MAX as u64 {
            buffer.extend_from_slice(&pack_u32(container_length as u32));
        } else {
            buffer.extend_from_slice(&pack_u32(0xFFFFFFFF));
        }
        buffer.extend_from_slice(&pack_u16(ContainerType::Data.to_code()));
        buffer.extend_from_slice(&pack_u16(operation.into()));
        buffer.extend_from_slice(&pack_u32(tx_id));

        // Collect all chunks into buffer
        while let Some(chunk_result) = data.next().await {
            let chunk = chunk_result.map_err(Error::Io)?;
            buffer.extend_from_slice(&chunk);
        }

        // Send entire data container as one USB transfer
        self.transport.send_bulk(&buffer).await?;

        // Receive response
        let response_bytes = self.transport.receive_bulk(512).await?;
        let response = ResponseContainer::from_bytes(&response_bytes)?;

        if response.transaction_id != tx_id {
            return Err(Error::invalid_data(format!(
                "Transaction ID mismatch: expected {}, got {}",
                tx_id, response.transaction_id
            )));
        }

        Ok(response)
    }

    /// Download an object as a stream of chunks.
    ///
    /// This is a convenience method that calls `execute_with_receive_stream`
    /// with GetObject operation.
    ///
    /// # Important
    ///
    /// The caller must consume the entire stream before calling any other
    /// session methods. The MTP session is locked while the stream is active.
    pub async fn get_object_stream(
        self: &Arc<Self>,
        handle: ObjectHandle,
    ) -> Result<ReceiveStream, Error> {
        self.execute_with_receive_stream(OperationCode::GetObject, &[handle.0])
            .await
    }

    /// Upload an object from a stream.
    ///
    /// This is a convenience method that streams object data directly to USB.
    ///
    /// # Arguments
    ///
    /// * `total_size` - Total bytes that will be sent
    /// * `data` - Stream of data chunks to send
    pub async fn send_object_stream<S>(&self, total_size: u64, data: S) -> Result<(), Error>
    where
        S: Stream<Item = Result<Bytes, std::io::Error>> + Unpin,
    {
        let response = self
            .execute_with_send_stream(OperationCode::SendObject, &[], total_size, data)
            .await?;
        Self::check_response(&response, OperationCode::SendObject)?;
        Ok(())
    }
}

/// A stream of data chunks received from USB during a download operation.
///
/// This stream yields `Bytes` chunks as they arrive from the device,
/// allowing memory-efficient streaming without buffering the entire file.
///
/// # Important
///
/// The MTP session is locked while this stream exists. You must consume
/// the entire stream (or drop it) before calling other session methods.
pub struct ReceiveStream {
    /// The transport layer for USB communication.
    transport: Arc<dyn Transport>,
    /// Guard that holds the operation lock for the duration of streaming.
    _guard: OwnedMutexGuard<()>,
    /// Transaction ID for this operation.
    transaction_id: u32,
    /// Operation code for this operation.
    operation: OperationCode,
    /// Buffer for partial container data.
    buffer: Vec<u8>,
    /// Total length of current container (from header).
    container_length: usize,
    /// How much payload we've already yielded from current container.
    payload_yielded: usize,
    /// Whether we've parsed the container header.
    header_parsed: bool,
    /// Whether the stream is complete.
    done: bool,
}

impl ReceiveStream {
    /// Get the transaction ID for this operation.
    #[must_use]
    pub fn transaction_id(&self) -> u32 {
        self.transaction_id
    }

    /// Poll for the next chunk of data.
    ///
    /// This is the async version of the Stream trait's poll_next.
    pub async fn next_chunk(&mut self) -> Option<Result<Bytes, Error>> {
        if self.done {
            return None;
        }

        loop {
            // If we have buffered data beyond what we've already yielded, yield it
            if self.header_parsed {
                let payload_start = HEADER_SIZE + self.payload_yielded;
                let payload_end = std::cmp::min(self.buffer.len(), self.container_length);

                if payload_start < payload_end {
                    // We have new data to yield
                    let chunk_data = self.buffer[payload_start..payload_end].to_vec();
                    self.payload_yielded += chunk_data.len();

                    // Check if this container is complete
                    if self.buffer.len() >= self.container_length {
                        // Remove this container from buffer
                        self.buffer.drain(..self.container_length);
                        self.header_parsed = false;
                        self.container_length = 0;
                        self.payload_yielded = 0;
                    }

                    if !chunk_data.is_empty() {
                        return Some(Ok(Bytes::from(chunk_data)));
                    }
                } else if self.buffer.len() >= self.container_length {
                    // Container complete but no new data (shouldn't happen, but handle it)
                    self.buffer.drain(..self.container_length);
                    self.header_parsed = false;
                    self.container_length = 0;
                    self.payload_yielded = 0;
                }
            }

            // Need more data from USB
            match self.transport.receive_bulk(64 * 1024).await {
                Ok(bytes) => {
                    if bytes.is_empty() {
                        return Some(Err(Error::invalid_data("Empty response from device")));
                    }
                    self.buffer.extend_from_slice(&bytes);
                }
                Err(e) => {
                    self.done = true;
                    return Some(Err(e));
                }
            }

            // Try to parse container header if we haven't yet
            if !self.header_parsed && self.buffer.len() >= HEADER_SIZE {
                let ct = match container_type(&self.buffer) {
                    Ok(ct) => ct,
                    Err(e) => {
                        self.done = true;
                        return Some(Err(e));
                    }
                };

                match ct {
                    ContainerType::Data => {
                        let length = match unpack_u32(&self.buffer[0..4]) {
                            Ok(l) => l as usize,
                            Err(e) => {
                                self.done = true;
                                return Some(Err(e));
                            }
                        };
                        self.container_length = length;
                        self.header_parsed = true;
                    }
                    ContainerType::Response => {
                        // End of data transfer
                        let response = match ResponseContainer::from_bytes(&self.buffer) {
                            Ok(r) => r,
                            Err(e) => {
                                self.done = true;
                                return Some(Err(e));
                            }
                        };

                        self.done = true;

                        // Check transaction ID
                        if response.transaction_id != self.transaction_id {
                            return Some(Err(Error::invalid_data(format!(
                                "Transaction ID mismatch: expected {}, got {}",
                                self.transaction_id, response.transaction_id
                            ))));
                        }

                        // Check response code
                        if response.code != ResponseCode::Ok {
                            return Some(Err(Error::Protocol {
                                code: response.code,
                                operation: self.operation,
                            }));
                        }

                        return None;
                    }
                    _ => {
                        self.done = true;
                        return Some(Err(Error::invalid_data(format!(
                            "Unexpected container type: {:?}",
                            ct
                        ))));
                    }
                }
            }
        }
    }

    /// Collect all remaining data into a `Vec<u8>`.
    ///
    /// This consumes the stream and buffers all data in memory.
    pub async fn collect(mut self) -> Result<Vec<u8>, Error> {
        let mut data = Vec::new();
        while let Some(result) = self.next_chunk().await {
            let chunk = result?;
            data.extend_from_slice(&chunk);
        }
        Ok(data)
    }
}

/// Convert a ReceiveStream into a futures::Stream using async iteration.
///
/// This creates a proper Stream that can be used with StreamExt methods.
pub fn receive_stream_to_stream(recv: ReceiveStream) -> impl Stream<Item = Result<Bytes, Error>> {
    futures::stream::unfold(recv, |mut recv| async move {
        recv.next_chunk().await.map(|result| (result, recv))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ptp::session::tests::{
        data_container, mock_transport, ok_response, response_with_params,
    };
    use crate::ptp::ResponseCode;

    #[tokio::test]
    async fn test_receive_stream_small_file() {
        let (transport, mock) = mock_transport();
        mock.queue_response(ok_response(1)); // OpenSession

        // GetObject data response (small file fits in one container)
        let file_data = vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
        mock.queue_response(data_container(2, OperationCode::GetObject, &file_data));
        mock.queue_response(ok_response(2));

        let session = Arc::new(PtpSession::open(transport, 1).await.unwrap());

        // Use streaming API
        let mut stream = session.get_object_stream(ObjectHandle(1)).await.unwrap();

        // Collect all chunks
        let mut received = Vec::new();
        while let Some(result) = stream.next_chunk().await {
            let chunk = result.unwrap();
            received.extend_from_slice(&chunk);
        }

        assert_eq!(received, file_data);
    }

    #[tokio::test]
    async fn test_receive_stream_collect() {
        let (transport, mock) = mock_transport();
        mock.queue_response(ok_response(1)); // OpenSession

        let file_data = vec![1, 2, 3, 4, 5];
        mock.queue_response(data_container(2, OperationCode::GetObject, &file_data));
        mock.queue_response(ok_response(2));

        let session = Arc::new(PtpSession::open(transport, 1).await.unwrap());

        let stream = session.get_object_stream(ObjectHandle(1)).await.unwrap();
        let collected = stream.collect().await.unwrap();

        assert_eq!(collected, file_data);
    }

    #[tokio::test]
    async fn test_receive_stream_error_response() {
        let (transport, mock) = mock_transport();
        mock.queue_response(ok_response(1)); // OpenSession

        // Return error response instead of data
        mock.queue_response(response_with_params(
            2,
            ResponseCode::InvalidObjectHandle,
            &[],
        ));

        let session = Arc::new(PtpSession::open(transport, 1).await.unwrap());

        let mut stream = session.get_object_stream(ObjectHandle(999)).await.unwrap();

        // Should get error when reading
        let result = stream.next_chunk().await;
        assert!(result.is_some());
        let err = result.unwrap();
        assert!(err.is_err());
    }

    #[tokio::test]
    async fn test_send_stream_small_file() {
        use futures::stream;

        let (transport, mock) = mock_transport();
        mock.queue_response(ok_response(1)); // OpenSession
        mock.queue_response(ok_response(2)); // SendObject response

        let session = PtpSession::open(transport, 1).await.unwrap();

        // Create a small data stream (use iter instead of once for Unpin)
        let data = vec![1u8, 2, 3, 4, 5];
        let data_stream = stream::iter(vec![Ok::<_, std::io::Error>(Bytes::from(data.clone()))]);

        // Send using streaming API
        session.send_object_stream(5, data_stream).await.unwrap();
    }

    #[tokio::test]
    async fn test_send_stream_multiple_chunks() {
        use futures::stream;

        let (transport, mock) = mock_transport();
        mock.queue_response(ok_response(1)); // OpenSession
        mock.queue_response(ok_response(2)); // SendObject response

        let session = PtpSession::open(transport, 1).await.unwrap();

        // Create a multi-chunk data stream
        let chunks = vec![
            Ok::<_, std::io::Error>(Bytes::from(vec![1, 2, 3])),
            Ok(Bytes::from(vec![4, 5, 6])),
            Ok(Bytes::from(vec![7, 8, 9, 10])),
        ];
        let data_stream = stream::iter(chunks);

        // Send using streaming API (total size = 10)
        session.send_object_stream(10, data_stream).await.unwrap();
    }

    #[tokio::test]
    async fn test_receive_stream_to_stream_conversion() {
        let (transport, mock) = mock_transport();
        mock.queue_response(ok_response(1)); // OpenSession

        let file_data = vec![1, 2, 3, 4, 5];
        mock.queue_response(data_container(2, OperationCode::GetObject, &file_data));
        mock.queue_response(ok_response(2));

        let session = Arc::new(PtpSession::open(transport, 1).await.unwrap());

        let recv_stream = session.get_object_stream(ObjectHandle(1)).await.unwrap();

        // Convert to futures::Stream and use StreamExt
        // Use pin_mut! to make it Unpin
        use futures::StreamExt;
        use std::pin::pin;
        let mut stream = pin!(receive_stream_to_stream(recv_stream));

        let mut collected = Vec::new();
        while let Some(result) = stream.next().await {
            collected.extend_from_slice(&result.unwrap());
        }

        assert_eq!(collected, file_data);
    }
}
