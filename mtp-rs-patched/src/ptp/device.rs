//! Low-level PTP device API.

use crate::ptp::{
    container_type, CommandContainer, ContainerType, DataContainer, DeviceInfo, OperationCode,
    PtpSession, ResponseCode, ResponseContainer,
};
use crate::transport::{NusbTransport, Transport};
use crate::Error;
use std::sync::Arc;
use std::time::Duration;

/// A low-level PTP device connection.
///
/// Use this for camera support or when you need raw PTP operations.
/// For typical MTP usage with Android devices, prefer `MtpDevice` instead.
pub struct PtpDevice {
    transport: Arc<NusbTransport>,
}

impl PtpDevice {
    /// Open a PTP device at a specific USB location (port).
    pub async fn open_by_location(location_id: u64) -> Result<Self, Error> {
        Self::open_by_location_with_timeout(location_id, NusbTransport::DEFAULT_TIMEOUT).await
    }

    /// Open by location with custom timeout.
    pub async fn open_by_location_with_timeout(
        location_id: u64,
        timeout: Duration,
    ) -> Result<Self, Error> {
        let devices = NusbTransport::list_mtp_devices()?;
        let device_info = devices
            .into_iter()
            .find(|d| d.location_id == location_id)
            .ok_or(Error::NoDevice)?;
        Self::open_device(device_info, timeout).await
    }

    /// Open a PTP device by its serial number.
    pub async fn open_by_serial(serial: &str) -> Result<Self, Error> {
        Self::open_by_serial_with_timeout(serial, NusbTransport::DEFAULT_TIMEOUT).await
    }

    /// Open by serial with custom timeout.
    pub async fn open_by_serial_with_timeout(
        serial: &str,
        timeout: Duration,
    ) -> Result<Self, Error> {
        let devices = NusbTransport::list_mtp_devices()?;
        let device_info = devices
            .into_iter()
            .find(|d| d.serial_number.as_deref() == Some(serial))
            .ok_or(Error::NoDevice)?;
        Self::open_device(device_info, timeout).await
    }

    /// Open the first available PTP device.
    pub async fn open_first() -> Result<Self, Error> {
        Self::open_first_with_timeout(NusbTransport::DEFAULT_TIMEOUT).await
    }

    /// Open the first available device with custom timeout.
    pub async fn open_first_with_timeout(timeout: Duration) -> Result<Self, Error> {
        let devices = NusbTransport::list_mtp_devices()?;
        let device_info = devices.into_iter().next().ok_or(Error::NoDevice)?;
        Self::open_device(device_info, timeout).await
    }

    async fn open_device(
        device_info: crate::transport::UsbDeviceInfo,
        timeout: Duration,
    ) -> Result<Self, Error> {
        let device = device_info.open().map_err(Error::Usb)?;
        let transport = NusbTransport::open_with_timeout(device, timeout).await?;
        Ok(Self {
            transport: Arc::new(transport),
        })
    }

    /// Get device info without opening a session.
    ///
    /// This is the only operation that can be performed without a session.
    pub async fn get_device_info(&self) -> Result<DeviceInfo, Error> {
        // Build GetDeviceInfo command (transaction ID 0 for session-less)
        let cmd = CommandContainer {
            code: OperationCode::GetDeviceInfo,
            transaction_id: 0,
            params: vec![],
        };
        self.transport.send_bulk(&cmd.to_bytes()).await?;

        // Receive data
        let mut data_payload = Vec::new();
        loop {
            let bytes = self.transport.receive_bulk(64 * 1024).await?;
            if bytes.is_empty() {
                return Err(Error::invalid_data("Empty response"));
            }

            let ct = container_type(&bytes)?;
            match ct {
                ContainerType::Data => {
                    let container = DataContainer::from_bytes(&bytes)?;
                    data_payload.extend_from_slice(&container.payload);
                }
                ContainerType::Response => {
                    let response = ResponseContainer::from_bytes(&bytes)?;
                    if response.code != ResponseCode::Ok {
                        return Err(Error::Protocol {
                            code: response.code,
                            operation: OperationCode::GetDeviceInfo,
                        });
                    }
                    break;
                }
                _ => {
                    return Err(Error::invalid_data(format!(
                        "Unexpected container type: {:?}",
                        ct
                    )));
                }
            }
        }

        DeviceInfo::from_bytes(&data_payload)
    }

    /// Open a PTP session.
    ///
    /// Most operations require a session to be open first.
    pub async fn open_session(&self) -> Result<PtpSession, Error> {
        self.open_session_with_id(1).await
    }

    /// Open a session with a specific session ID.
    pub async fn open_session_with_id(&self, session_id: u32) -> Result<PtpSession, Error> {
        let transport: Arc<dyn Transport> = self.transport.clone();
        PtpSession::open(transport, session_id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore] // Requires real device
    async fn test_open_first() {
        let device = PtpDevice::open_first().await.unwrap();
        let info = device.get_device_info().await.unwrap();
        println!("Model: {}", info.model);
    }

    #[tokio::test]
    #[ignore] // Requires real device
    async fn test_open_session() {
        let device = PtpDevice::open_first().await.unwrap();
        let session = device.open_session().await.unwrap();

        let info = session.get_device_info().await.unwrap();
        println!("Model: {}", info.model);

        session.close().await.unwrap();
    }
}
