//! USB transport implementation using nusb.

use super::Transport;
use async_trait::async_trait;
use futures::lock::Mutex;
use futures_timer::Delay;
use nusb::descriptors::{InterfaceDescriptor, TransferType};
use nusb::transfer::{Buffer, Bulk, Direction, In, Interrupt, Out, TransferError};
use nusb::MaybeFuture;
use std::time::Duration;

/// MTP interface class code (Still Image).
const MTP_CLASS_IMAGE: u8 = 0x06;
/// MTP interface class code (Vendor-specific).
const MTP_CLASS_VENDOR: u8 = 0xFF;
/// MTP subclass code.
const MTP_SUBCLASS: u8 = 0x01;
/// MTP protocol code (PTP).
const MTP_PROTOCOL: u8 = 0x01;

/// USB device information with topology-based location ID.
#[derive(Debug, Clone)]
pub struct UsbDeviceInfo {
    /// USB vendor ID
    pub vendor_id: u16,
    /// USB product ID
    pub product_id: u16,
    /// Manufacturer name (e.g., "Google", "Samsung")
    pub manufacturer: Option<String>,
    /// Product name (e.g., "Pixel 9 Pro XL")
    pub product: Option<String>,
    /// Device serial number (if available)
    pub serial_number: Option<String>,
    /// USB location identifier derived from bus and port topology (stable per port)
    pub location_id: u64,
    /// Reference to the underlying nusb device info for opening
    nusb_info: nusb::DeviceInfo,
}

impl UsbDeviceInfo {
    /// Open the USB device.
    pub fn open(&self) -> Result<nusb::Device, nusb::Error> {
        self.nusb_info.open().wait()
    }
}

/// USB transport implementation using nusb.
pub struct NusbTransport {
    bulk_in: Mutex<nusb::Endpoint<Bulk, In>>,
    bulk_out: Mutex<nusb::Endpoint<Bulk, Out>>,
    interrupt_in: Mutex<nusb::Endpoint<Interrupt, In>>,
    /// Timeout for bulk transfers (sending commands, receiving data).
    timeout: Duration,
}

impl NusbTransport {
    /// Default timeout for bulk transfers (30 seconds for large file transfers).
    pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

    /// Default buffer size for interrupt transfers.
    const INTERRUPT_BUFFER_SIZE: usize = 64;

    /// List all available MTP devices with location IDs.
    pub fn list_mtp_devices() -> Result<Vec<UsbDeviceInfo>, crate::Error> {
        let devices = nusb::list_devices()
            .wait()
            .map_err(crate::Error::Usb)?
            .filter(Self::is_mtp_device)
            .map(|dev| {
                let location_id = location_id_from_topology(&dev);
                UsbDeviceInfo {
                    vendor_id: dev.vendor_id(),
                    product_id: dev.product_id(),
                    manufacturer: dev.manufacturer_string().map(String::from),
                    product: dev.product_string().map(String::from),
                    serial_number: dev.serial_number().map(String::from),
                    location_id,
                    nusb_info: dev,
                }
            })
            .collect();
        Ok(devices)
    }

    /// Check if a device info represents an MTP device.
    fn is_mtp_device(dev: &nusb::DeviceInfo) -> bool {
        // Check device class/subclass/protocol at device level.
        if Self::is_mtp_class(dev.class(), dev.subclass(), dev.protocol()) {
            return true;
        }

        // Many devices are composite (class 0) or vendor-specific (class 0xFF)
        // with MTP on one interface. Only inspect these further.
        if dev.class() != 0 && dev.class() != MTP_CLASS_VENDOR {
            return false;
        }

        // Check interface-level class info available from DeviceInfo without opening.
        for intf in dev.interfaces() {
            if Self::is_mtp_class(intf.class(), intf.subclass(), intf.protocol()) {
                return true;
            }
        }

        // Fall back to opening the device and inspecting full configuration descriptors.
        // This also catches vendor-specific interfaces (class 0xFF) that use non-standard
        // subclass/protocol but have the MTP endpoint layout (e.g. Amazon Kindle).
        if let Ok(device) = dev.open().wait() {
            if let Ok(config) = device.active_configuration() {
                for interface in config.interfaces() {
                    if let Some(alt) = interface.alt_settings().next() {
                        if Self::is_mtp_interface(&alt) {
                            return true;
                        }
                    }
                }
            }
        }

        false
    }

    /// Check if class/subclass/protocol match standard MTP identifiers.
    fn is_mtp_class(class: u8, subclass: u8, protocol: u8) -> bool {
        (class == MTP_CLASS_IMAGE || class == MTP_CLASS_VENDOR)
            && subclass == MTP_SUBCLASS
            && protocol == MTP_PROTOCOL
    }

    /// Check if an interface descriptor looks like an MTP interface.
    ///
    /// Matches standard MTP class/subclass/protocol, and also vendor-specific
    /// interfaces (class 0xFF) with non-standard subclass/protocol that have
    /// the MTP endpoint layout (bulk IN + bulk OUT + interrupt IN). Some devices
    /// like Amazon Kindle use vendor-specific descriptors while still speaking MTP.
    fn is_mtp_interface(alt: &InterfaceDescriptor) -> bool {
        if Self::is_mtp_class(alt.class(), alt.subclass(), alt.protocol()) {
            return true;
        }
        // For vendor-specific class, subclass and protocol are vendor-defined,
        // so we can't rely on them. Use endpoint layout as a heuristic instead.
        alt.class() == MTP_CLASS_VENDOR && Self::has_mtp_endpoint_layout(alt)
    }

    /// Check if an interface has the MTP endpoint layout:
    /// one bulk IN, one bulk OUT, and one interrupt IN endpoint.
    fn has_mtp_endpoint_layout(alt: &InterfaceDescriptor) -> bool {
        let mut bulk_in = false;
        let mut bulk_out = false;
        let mut interrupt_in = false;
        for ep in alt.endpoints() {
            match (ep.direction(), ep.transfer_type()) {
                (Direction::In, TransferType::Bulk) => bulk_in = true,
                (Direction::Out, TransferType::Bulk) => bulk_out = true,
                (Direction::In, TransferType::Interrupt) => interrupt_in = true,
                _ => {}
            }
        }
        bulk_in && bulk_out && interrupt_in
    }

    /// Open a specific device and claim the MTP interface.
    pub async fn open(device: nusb::Device) -> Result<Self, crate::Error> {
        Self::open_with_timeout(device, Self::DEFAULT_TIMEOUT).await
    }

    /// Open with custom bulk transfer timeout.
    pub async fn open_with_timeout(
        device: nusb::Device,
        timeout: Duration,
    ) -> Result<Self, crate::Error> {
        // Find the MTP interface
        let config = device.active_configuration().map_err(|e| {
            crate::Error::invalid_data(format!("Failed to get configuration: {}", e))
        })?;

        let mut mtp_interface_number = None;
        let mut bulk_in_addr = None;
        let mut bulk_out_addr = None;
        let mut interrupt_in_addr = None;

        for interface in config.interfaces() {
            // Get the first alternate setting for this interface
            let Some(alt_setting) = interface.alt_settings().next() else {
                continue;
            };

            // Check if this interface is MTP (standard or vendor-specific with MTP endpoints)
            if Self::is_mtp_interface(&alt_setting) {
                mtp_interface_number = Some(interface.interface_number());

                // Find endpoints
                for endpoint in alt_setting.endpoints() {
                    match (endpoint.direction(), endpoint.transfer_type()) {
                        (Direction::Out, TransferType::Bulk) => {
                            bulk_out_addr = Some(endpoint.address());
                        }
                        (Direction::In, TransferType::Bulk) => {
                            bulk_in_addr = Some(endpoint.address());
                        }
                        (Direction::In, TransferType::Interrupt) => {
                            interrupt_in_addr = Some(endpoint.address());
                        }
                        _ => {}
                    }
                }

                break;
            }
        }

        let interface_number = mtp_interface_number
            .ok_or_else(|| crate::Error::invalid_data("No MTP interface found on device"))?;

        let bulk_in_addr =
            bulk_in_addr.ok_or_else(|| crate::Error::invalid_data("No bulk IN endpoint found"))?;
        let bulk_out_addr = bulk_out_addr
            .ok_or_else(|| crate::Error::invalid_data("No bulk OUT endpoint found"))?;
        let interrupt_in_addr = interrupt_in_addr
            .ok_or_else(|| crate::Error::invalid_data("No interrupt IN endpoint found"))?;

        // Claim the interface
        let interface = device
            .claim_interface(interface_number)
            .wait()
            .map_err(crate::Error::Usb)?;

        // Open endpoints
        let bulk_in = interface
            .endpoint::<Bulk, In>(bulk_in_addr)
            .map_err(crate::Error::Usb)?;
        let bulk_out = interface
            .endpoint::<Bulk, Out>(bulk_out_addr)
            .map_err(crate::Error::Usb)?;
        let interrupt_in = interface
            .endpoint::<Interrupt, In>(interrupt_in_addr)
            .map_err(crate::Error::Usb)?;

        Ok(Self {
            bulk_in: Mutex::new(bulk_in),
            bulk_out: Mutex::new(bulk_out),
            interrupt_in: Mutex::new(interrupt_in),
            timeout,
        })
    }

    /// Get the bulk transfer timeout.
    #[must_use]
    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    /// Set the bulk transfer timeout.
    pub fn set_timeout(&mut self, timeout: Duration) {
        self.timeout = timeout;
    }

    /// Convert a nusb TransferError to crate::Error.
    fn convert_transfer_error(err: TransferError) -> crate::Error {
        match err {
            // send_bulk uses transfer_blocking, which cancels the transfer on
            // timeout and returns Cancelled. Map to Timeout so that
            // Error::is_retryable() treats it correctly.
            TransferError::Cancelled => crate::Error::Timeout,
            TransferError::Disconnected => crate::Error::Disconnected,
            TransferError::Stall
            | TransferError::Fault
            | TransferError::InvalidArgument
            | TransferError::Unknown(_) => crate::Error::Io(std::io::Error::other(err.to_string())),
        }
    }
}

#[async_trait]
impl Transport for NusbTransport {
    async fn send_bulk(&self, data: &[u8]) -> Result<(), crate::Error> {
        let mut ep = self.bulk_out.lock().await;
        let buf: Buffer = data.to_vec().into();
        let completion = ep.transfer_blocking(buf, self.timeout);
        completion.status.map_err(Self::convert_transfer_error)?;
        Ok(())
    }

    async fn receive_bulk(&self, max_size: usize) -> Result<Vec<u8>, crate::Error> {
        let mut ep = self.bulk_in.lock().await;

        // If there's no pending transfer from a previous timed-out call,
        // submit a new one. Otherwise, the pending transfer already has our
        // data in flight and we just need to wait for it.
        if ep.pending() == 0 {
            let max_packet_size = ep.max_packet_size();
            let aligned_size = align_to_packet_size(max_size, max_packet_size);
            ep.submit(Buffer::new(aligned_size));
        }

        // Wait for the transfer to complete OR the timeout to expire.
        // next_complete() is cancel-safe: dropping its future does NOT cancel
        // the underlying USB transfer. On timeout we leave the transfer pending
        // so a subsequent call picks up the in-flight data.
        let completion = futures::future::select(
            Box::pin(ep.next_complete()),
            Box::pin(Delay::new(self.timeout)),
        )
        .await;

        match completion {
            futures::future::Either::Left((comp, _)) => {
                comp.status.map_err(Self::convert_transfer_error)?;
                Ok(comp.buffer[..comp.actual_len].to_vec())
            }
            futures::future::Either::Right((_, _)) => {
                // Don't cancel the transfer — it stays pending in the endpoint.
                // next_complete() is cancel-safe, so dropping its future is fine.
                // On retry, the next call will find pending() > 0 and pick it up.
                Err(crate::Error::Timeout)
            }
        }
    }

    async fn receive_interrupt(&self) -> Result<Vec<u8>, crate::Error> {
        let mut ep = self.interrupt_in.lock().await;

        // Submit a new transfer only if none is already pending.
        if ep.pending() == 0 {
            let max_packet_size = ep.max_packet_size();
            let aligned_size = align_to_packet_size(Self::INTERRUPT_BUFFER_SIZE, max_packet_size);
            ep.submit(Buffer::new(aligned_size));
        }

        // Await indefinitely — callers handle cancellation via async
        // cancellation (e.g. tokio::time::timeout or select!).
        let completion = ep.next_complete().await;
        completion.status.map_err(Self::convert_transfer_error)?;
        Ok(completion.buffer[..completion.actual_len].to_vec())
    }
}

/// Round `size` up to the nearest multiple of `packet_size`.
///
/// nusb 0.2 requires that IN transfer buffer sizes are non-zero multiples of
/// the endpoint's maximum packet size.
fn align_to_packet_size(size: usize, packet_size: usize) -> usize {
    if packet_size == 0 {
        return size.max(1);
    }
    if size == 0 {
        return packet_size;
    }
    if size % packet_size == 0 {
        size
    } else {
        ((size / packet_size) + 1) * packet_size
    }
}

/// Derive a stable location identifier from USB topology (bus + port chain).
///
/// Uses FNV-1a to hash `bus_id` and `port_chain` into a deterministic `u64`.
/// The result is stable across calls for the same physical USB port, regardless
/// of which device is plugged in.
fn location_id_from_topology(dev: &nusb::DeviceInfo) -> u64 {
    // FNV-1a 64-bit constants
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0100_0000_01b3;

    let mut hash = FNV_OFFSET;
    for byte in dev.bus_id().as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    // Separator so bus_id "1" + port [2,3] differs from bus_id "12" + port [3]
    hash ^= 0xFF;
    hash = hash.wrapping_mul(FNV_PRIME);
    for byte in dev.port_chain() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore] // Requires real MTP device
    fn test_list_devices() {
        let devices = NusbTransport::list_mtp_devices().unwrap();
        println!("Found {} MTP devices", devices.len());
        for dev in &devices {
            println!(
                "  {:04x}:{:04x} serial={:?} location={:08x}",
                dev.vendor_id, dev.product_id, dev.serial_number, dev.location_id,
            );
        }
    }

    #[tokio::test]
    #[ignore] // Requires real MTP device
    async fn test_open_device() {
        let devices = NusbTransport::list_mtp_devices().unwrap();
        assert!(!devices.is_empty(), "No MTP device found");

        let device = devices[0].open().unwrap();
        let transport = NusbTransport::open(device).await.unwrap();

        assert_eq!(transport.timeout(), NusbTransport::DEFAULT_TIMEOUT);
    }

    #[tokio::test]
    #[ignore] // Requires real MTP device
    async fn test_timeout_configuration() {
        let devices = NusbTransport::list_mtp_devices().unwrap();
        assert!(!devices.is_empty(), "No MTP device found");

        let device = devices[0].open().unwrap();
        let custom_timeout = Duration::from_secs(60);
        let mut transport = NusbTransport::open_with_timeout(device, custom_timeout)
            .await
            .unwrap();

        assert_eq!(transport.timeout(), custom_timeout);

        // Test setter
        let new_timeout = Duration::from_secs(10);
        transport.set_timeout(new_timeout);
        assert_eq!(transport.timeout(), new_timeout);
    }

    #[test]
    fn test_align_to_packet_size() {
        // Zero size rounds up to packet_size
        assert_eq!(align_to_packet_size(0, 512), 512);
        // Size smaller than packet rounds up
        assert_eq!(align_to_packet_size(1, 512), 512);
        // Exact multiple stays the same
        assert_eq!(align_to_packet_size(512, 512), 512);
        assert_eq!(align_to_packet_size(1024, 512), 1024);
        // Non-multiple rounds up
        assert_eq!(align_to_packet_size(513, 512), 1024);
        assert_eq!(align_to_packet_size(100, 64), 128);
        // Zero packet_size edge case
        assert_eq!(align_to_packet_size(0, 0), 1);
        assert_eq!(align_to_packet_size(100, 0), 100);
    }

    #[test]
    fn test_mtp_class_detection() {
        // Image class with MTP subclass/protocol
        assert!(NusbTransport::is_mtp_class(0x06, 0x01, 0x01));

        // Vendor class with MTP subclass/protocol
        assert!(NusbTransport::is_mtp_class(0xFF, 0x01, 0x01));

        // Wrong class
        assert!(!NusbTransport::is_mtp_class(0x08, 0x01, 0x01));

        // Wrong subclass
        assert!(!NusbTransport::is_mtp_class(0x06, 0x00, 0x01));

        // Wrong protocol
        assert!(!NusbTransport::is_mtp_class(0x06, 0x01, 0x00));

        // Vendor-specific with non-standard subclass/protocol (e.g. Kindle ff/ff/00)
        assert!(!NusbTransport::is_mtp_class(0xFF, 0xFF, 0x00));
    }
}
