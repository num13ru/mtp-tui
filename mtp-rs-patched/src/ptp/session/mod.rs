//! PTP session management.
//!
//! This module provides session-level operations for MTP/PTP communication.
//! A session maintains the connection state and serializes concurrent operations.

mod operations;
mod properties;
mod streaming;

pub use streaming::{receive_stream_to_stream, ReceiveStream};

use crate::ptp::{
    container_type, unpack_u32, CommandContainer, ContainerType, DataContainer, OperationCode,
    ResponseCode, ResponseContainer, SessionId, TransactionId,
};
use crate::transport::Transport;
use crate::Error;
use futures::lock::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

/// Container header size in bytes.
pub(crate) const HEADER_SIZE: usize = 12;

/// A PTP session with a device.
///
/// PtpSession manages the lifecycle of a PTP/MTP session, including:
/// - Opening and closing sessions
/// - Transaction ID management
/// - Serializing concurrent operations (MTP only allows one operation at a time)
/// - Executing operations and receiving responses
///
/// # Example
///
/// ```rust,ignore
/// use mtp_rs::ptp::PtpSession;
///
/// // Open a session with session ID 1
/// let session = PtpSession::open(transport, 1).await?;
///
/// // Get device info
/// let device_info = session.get_device_info().await?;
///
/// // Get storage IDs
/// let storage_ids = session.get_storage_ids().await?;
///
/// // Close the session when done
/// session.close().await?;
/// ```
pub struct PtpSession {
    /// The transport layer for USB communication.
    pub(crate) transport: Arc<dyn Transport>,
    /// The session ID assigned to this session.
    session_id: SessionId,
    /// Atomic counter for generating transaction IDs.
    transaction_id: AtomicU32,
    /// Mutex to serialize operations (MTP only allows one operation at a time).
    /// Wrapped in Arc so it can be shared with ReceiveStream.
    pub(crate) operation_lock: Arc<Mutex<()>>,
}

impl PtpSession {
    /// Create a new session (internal, use open() to start session).
    fn new(transport: Arc<dyn Transport>, session_id: SessionId) -> Self {
        Self {
            transport,
            session_id,
            transaction_id: AtomicU32::new(TransactionId::FIRST.0),
            operation_lock: Arc::new(Mutex::new(())),
        }
    }

    /// Open a new session with the device.
    ///
    /// This sends an OpenSession command to the device and establishes a session
    /// with the given session ID.
    ///
    /// # Arguments
    ///
    /// * `transport` - The transport layer for USB communication
    /// * `session_id` - The session ID to use (typically 1)
    ///
    /// # Errors
    ///
    /// Returns an error if the device rejects the session or communication fails.
    pub async fn open(transport: Arc<dyn Transport>, session_id: u32) -> Result<Self, Error> {
        let session = Self::new(transport, SessionId(session_id));

        // PTP spec: OpenSession is a session-less operation, so use tx_id=0.
        // Some devices (e.g. Amazon Kindle) enforce this strictly and reject
        // OpenSession with tx_id != 0 as InvalidParameter.
        let response = Self::send_open_session(&session.transport, session_id).await?;

        if response.code == ResponseCode::Ok {
            return Ok(session);
        }

        if response.code == ResponseCode::SessionAlreadyOpen {
            let _ = session.execute(OperationCode::CloseSession, &[]).await;

            let fresh_session = Self::new(Arc::clone(&session.transport), SessionId(session_id));

            let retry_response =
                Self::send_open_session(&fresh_session.transport, session_id).await?;

            if retry_response.code != ResponseCode::Ok {
                return Err(Error::Protocol {
                    code: retry_response.code,
                    operation: OperationCode::OpenSession,
                });
            }

            return Ok(fresh_session);
        }

        Err(Error::Protocol {
            code: response.code,
            operation: OperationCode::OpenSession,
        })
    }

    /// Send OpenSession with transaction_id=0 (SESSION_LESS) per the PTP spec.
    async fn send_open_session(
        transport: &Arc<dyn Transport>,
        session_id: u32,
    ) -> Result<ResponseContainer, Error> {
        let cmd = CommandContainer {
            code: OperationCode::OpenSession,
            transaction_id: TransactionId::SESSION_LESS.0,
            params: vec![session_id],
        };
        transport.send_bulk(&cmd.to_bytes()).await?;

        let response_bytes = transport.receive_bulk(512).await?;
        ResponseContainer::from_bytes(&response_bytes)
    }

    /// Get the session ID.
    #[must_use]
    pub fn session_id(&self) -> SessionId {
        self.session_id
    }

    /// Close the session.
    ///
    /// This sends a CloseSession command to the device. Errors during close
    /// are ignored since the session is being terminated anyway.
    pub async fn close(self) -> Result<(), Error> {
        let _ = self.execute(OperationCode::CloseSession, &[]).await;
        Ok(())
    }

    /// Get the next transaction ID.
    ///
    /// Transaction IDs start at 1 and wrap correctly, skipping 0 and 0xFFFFFFFF.
    pub(crate) fn next_transaction_id(&self) -> u32 {
        loop {
            let current = self.transaction_id.load(Ordering::SeqCst);
            let next = TransactionId(current).next().0;
            if self
                .transaction_id
                .compare_exchange(current, next, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
            {
                return current;
            }
        }
    }

    // =========================================================================
    // Core operation execution
    // =========================================================================

    /// Execute an operation without data phase.
    pub(crate) async fn execute(
        &self,
        operation: OperationCode,
        params: &[u32],
    ) -> Result<ResponseContainer, Error> {
        let _guard = self.operation_lock.lock().await;

        let tx_id = self.next_transaction_id();

        // Build and send command
        let cmd = CommandContainer {
            code: operation,
            transaction_id: tx_id,
            params: params.to_vec(),
        };
        self.transport.send_bulk(&cmd.to_bytes()).await?;

        // Receive response
        let response_bytes = self.transport.receive_bulk(512).await?;
        let response = ResponseContainer::from_bytes(&response_bytes)?;

        // Verify transaction ID matches
        if response.transaction_id != tx_id {
            return Err(Error::invalid_data(format!(
                "Transaction ID mismatch: expected {}, got {}",
                tx_id, response.transaction_id
            )));
        }

        Ok(response)
    }

    /// Execute operation with data receive phase.
    pub(crate) async fn execute_with_receive(
        &self,
        operation: OperationCode,
        params: &[u32],
    ) -> Result<(ResponseContainer, Vec<u8>), Error> {
        let _guard = self.operation_lock.lock().await;

        let tx_id = self.next_transaction_id();

        // Send command
        let cmd = CommandContainer {
            code: operation,
            transaction_id: tx_id,
            params: params.to_vec(),
        };
        self.transport.send_bulk(&cmd.to_bytes()).await?;

        // Receive data container(s)
        // MTP sends data in one or more containers, then response.
        // A single data container may span multiple USB transfers if larger than 64KB.
        let mut data = Vec::new();

        loop {
            let mut bytes = self.transport.receive_bulk(64 * 1024).await?;
            if bytes.is_empty() {
                return Err(Error::invalid_data("Empty response"));
            }

            let ct = container_type(&bytes)?;
            match ct {
                ContainerType::Data => {
                    // Check if we need to receive more data for this container.
                    // The length field in the header tells us the total container size.
                    if bytes.len() >= 4 {
                        let total_length = unpack_u32(&bytes[0..4])? as usize;
                        // Keep receiving until we have the complete container
                        while bytes.len() < total_length {
                            let more = self.transport.receive_bulk(64 * 1024).await?;
                            if more.is_empty() {
                                return Err(Error::invalid_data(
                                    "Incomplete data container: device stopped sending",
                                ));
                            }
                            bytes.extend_from_slice(&more);
                        }
                    }
                    let container = DataContainer::from_bytes(&bytes)?;
                    data.extend_from_slice(&container.payload);
                    // Continue to receive more containers or response
                }
                ContainerType::Response => {
                    let response = ResponseContainer::from_bytes(&bytes)?;
                    if response.transaction_id != tx_id {
                        return Err(Error::invalid_data(format!(
                            "Transaction ID mismatch: expected {}, got {}",
                            tx_id, response.transaction_id
                        )));
                    }
                    return Ok((response, data));
                }
                _ => {
                    return Err(Error::invalid_data(format!(
                        "Unexpected container type: {:?}",
                        ct
                    )));
                }
            }
        }
    }

    /// Execute operation with data send phase.
    pub(crate) async fn execute_with_send(
        &self,
        operation: OperationCode,
        params: &[u32],
        data: &[u8],
    ) -> Result<ResponseContainer, Error> {
        let _guard = self.operation_lock.lock().await;

        let tx_id = self.next_transaction_id();

        // Send command
        let cmd = CommandContainer {
            code: operation,
            transaction_id: tx_id,
            params: params.to_vec(),
        };
        self.transport.send_bulk(&cmd.to_bytes()).await?;

        // Send data
        let data_container = DataContainer {
            code: operation,
            transaction_id: tx_id,
            payload: data.to_vec(),
        };
        self.transport.send_bulk(&data_container.to_bytes()).await?;

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

    // =========================================================================
    // Helper methods
    // =========================================================================

    /// Helper to check response is OK.
    pub(crate) fn check_response(
        response: &ResponseContainer,
        operation: OperationCode,
    ) -> Result<(), Error> {
        if response.code == ResponseCode::Ok {
            Ok(())
        } else {
            Err(Error::Protocol {
                code: response.code,
                operation,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ptp::{pack_u16, pack_u32, ContainerType, ObjectHandle};
    use crate::transport::mock::MockTransport;

    /// Create a mock transport as Arc<dyn Transport>.
    pub(crate) fn mock_transport() -> (Arc<dyn Transport>, Arc<MockTransport>) {
        let mock = Arc::new(MockTransport::new());
        let transport: Arc<dyn Transport> = Arc::clone(&mock) as Arc<dyn Transport>;
        (transport, mock)
    }

    /// Build an OK response container bytes.
    pub(crate) fn ok_response(tx_id: u32) -> Vec<u8> {
        let mut buf = Vec::with_capacity(12);
        buf.extend_from_slice(&pack_u32(12)); // length
        buf.extend_from_slice(&pack_u16(ContainerType::Response.to_code()));
        buf.extend_from_slice(&pack_u16(ResponseCode::Ok.into()));
        buf.extend_from_slice(&pack_u32(tx_id));
        buf
    }

    /// Build a response container with params.
    pub(crate) fn response_with_params(tx_id: u32, code: ResponseCode, params: &[u32]) -> Vec<u8> {
        let len = 12 + params.len() * 4;
        let mut buf = Vec::with_capacity(len);
        buf.extend_from_slice(&pack_u32(len as u32));
        buf.extend_from_slice(&pack_u16(ContainerType::Response.to_code()));
        buf.extend_from_slice(&pack_u16(code.into()));
        buf.extend_from_slice(&pack_u32(tx_id));
        for p in params {
            buf.extend_from_slice(&pack_u32(*p));
        }
        buf
    }

    /// Build a data container.
    pub(crate) fn data_container(tx_id: u32, code: OperationCode, payload: &[u8]) -> Vec<u8> {
        let len = 12 + payload.len();
        let mut buf = Vec::with_capacity(len);
        buf.extend_from_slice(&pack_u32(len as u32));
        buf.extend_from_slice(&pack_u16(ContainerType::Data.to_code()));
        buf.extend_from_slice(&pack_u16(code.into()));
        buf.extend_from_slice(&pack_u32(tx_id));
        buf.extend_from_slice(payload);
        buf
    }

    #[tokio::test]
    async fn test_open_session() {
        let (transport, mock) = mock_transport();
        mock.queue_response(ok_response(1));

        let session = PtpSession::open(transport, 1).await.unwrap();
        assert_eq!(session.session_id(), SessionId(1));
    }

    #[tokio::test]
    async fn test_open_session_already_open_recovers() {
        let (transport, mock) = mock_transport();

        // First OpenSession returns SessionAlreadyOpen
        mock.queue_response(response_with_params(
            1,
            ResponseCode::SessionAlreadyOpen,
            &[],
        ));
        // CloseSession response (ignored, but we need to provide one)
        mock.queue_response(ok_response(2));
        // Second OpenSession (fresh session, tx_id starts at 1 again)
        mock.queue_response(ok_response(1));

        // Should succeed by closing and reopening
        let session = PtpSession::open(transport, 1).await.unwrap();
        assert_eq!(session.session_id(), SessionId(1));
    }

    #[tokio::test]
    async fn test_open_session_already_open_transaction_id_reset() {
        let (transport, mock) = mock_transport();

        // First OpenSession returns SessionAlreadyOpen
        mock.queue_response(response_with_params(
            1,
            ResponseCode::SessionAlreadyOpen,
            &[],
        ));
        // CloseSession response
        mock.queue_response(ok_response(2));
        // Second OpenSession (fresh session, tx_id starts at 1 again)
        mock.queue_response(ok_response(1));
        // Next operation should use tx_id = 2 (after the fresh OpenSession used 1)
        mock.queue_response(ok_response(2));

        let session = PtpSession::open(transport, 1).await.unwrap();

        // Perform an operation to verify transaction ID is properly reset
        // The next operation should use tx_id = 2 (since the fresh OpenSession used 1)
        session.delete_object(ObjectHandle(1)).await.unwrap();
    }

    #[tokio::test]
    async fn test_open_session_already_open_close_error_ignored() {
        let (transport, mock) = mock_transport();

        // First OpenSession returns SessionAlreadyOpen
        mock.queue_response(response_with_params(
            1,
            ResponseCode::SessionAlreadyOpen,
            &[],
        ));
        // CloseSession returns an error (should be ignored)
        mock.queue_response(response_with_params(2, ResponseCode::GeneralError, &[]));
        // Second OpenSession succeeds
        mock.queue_response(ok_response(1));

        // Should succeed even if CloseSession fails
        let session = PtpSession::open(transport, 1).await.unwrap();
        assert_eq!(session.session_id(), SessionId(1));
    }

    #[tokio::test]
    async fn test_open_session_error() {
        let (transport, mock) = mock_transport();
        mock.queue_response(response_with_params(1, ResponseCode::GeneralError, &[]));

        let result = PtpSession::open(transport, 1).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_transaction_id_increment() {
        let (transport, mock) = mock_transport();
        mock.queue_response(ok_response(1)); // OpenSession
        mock.queue_response(ok_response(2)); // First operation
        mock.queue_response(ok_response(3)); // Second operation

        let session = PtpSession::open(transport, 1).await.unwrap();

        // Execute two operations and verify transaction IDs increment
        session.delete_object(ObjectHandle(1)).await.unwrap();
        session.delete_object(ObjectHandle(2)).await.unwrap();
    }

    #[tokio::test]
    async fn test_transaction_id_mismatch() {
        let (transport, mock) = mock_transport();
        mock.queue_response(ok_response(1)); // OpenSession
        mock.queue_response(ok_response(999)); // Wrong transaction ID

        let session = PtpSession::open(transport, 1).await.unwrap();
        let result = session.delete_object(ObjectHandle(1)).await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_close_session() {
        let (transport, mock) = mock_transport();
        mock.queue_response(ok_response(1)); // OpenSession
        mock.queue_response(ok_response(2)); // CloseSession

        let session = PtpSession::open(transport, 1).await.unwrap();
        session.close().await.unwrap();
    }

    #[tokio::test]
    async fn test_close_session_ignores_errors() {
        let (transport, mock) = mock_transport();
        mock.queue_response(ok_response(1)); // OpenSession
        mock.queue_response(response_with_params(2, ResponseCode::GeneralError, &[])); // CloseSession error

        let session = PtpSession::open(transport, 1).await.unwrap();
        // Should succeed even if close fails
        session.close().await.unwrap();
    }
}
