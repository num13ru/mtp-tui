//! USB transport abstraction layer.

#[cfg(test)]
pub mod mock;
pub mod nusb;

pub use self::nusb::{NusbTransport, UsbDeviceInfo};

use async_trait::async_trait;

/// Transport trait for MTP/PTP communication.
///
/// Abstracts USB communication to enable testing with mock transport.
#[async_trait]
pub trait Transport: Send + Sync {
    /// Send data on the bulk OUT endpoint.
    async fn send_bulk(&self, data: &[u8]) -> Result<(), crate::Error>;

    /// Receive data from the bulk IN endpoint.
    ///
    /// `max_size` is the maximum bytes to receive in one call.
    async fn receive_bulk(&self, max_size: usize) -> Result<Vec<u8>, crate::Error>;

    /// Receive event data from the interrupt IN endpoint.
    ///
    /// This may block until an event is available.
    async fn receive_interrupt(&self) -> Result<Vec<u8>, crate::Error>;
}
