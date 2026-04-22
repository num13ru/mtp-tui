# mtp-rs Missing MTP Codes

Codes that the Kindle Paperwhite advertises but mtp-rs currently maps to `Unknown(...)`.
Derived from `log/mtp_capabilities.log` and cross-referenced against the MTP 1.1 spec
and Android's `mtp.h`.

This document serves as a patch backlog for mtp-rs.

## Missing Operation Codes (`OperationCode` enum)

mtp-rs currently defines `GetObjectPropValue` (0x9803) and `SetObjectPropValue` (0x9804)
but none of the other MTP object-property operations.

| Hex    | Shown as        | MTP Spec Name             | Description                                                        | Priority    |
|--------|-----------------|---------------------------|--------------------------------------------------------------------|-------------|
| 0x9801 | Unknown(38913)  | `GetObjectPropsSupported` | Returns `u16[]` of supported property codes for a given format     | **HIGH**    |
| 0x9802 | Unknown(38914)  | `GetObjectPropDesc`       | Returns property descriptor (data type, default, range) per prop   | **HIGH**    |
| 0x9805 | Unknown(38917)  | `GetObjectPropList`       | Batch-fetch multiple properties in one round-trip                  | MEDIUM      |
| 0x9806 | Unknown(38918)  | `SetObjectPropList`       | Batch-set multiple property values                                 | LOW         |
| 0x9808 | Unknown(38920)  | `SendObjectPropList`      | Send property values alongside object during upload                | LOW         |
| 0x9810 | Unknown(38928)  | `GetObjectReferences`     | Get playlist/album member handles                                  | LOW         |
| 0x9811 | Unknown(38929)  | `SetObjectReferences`     | Set playlist/album member handles                                  | LOW         |
| 0x1011 | Unknown(4113)   | `SelfTest`                | PTP standard device self-test                                      | NONE        |
| 0x9202 | Unknown(37378)  | WMDRMPD vendor op         | Microsoft DRM extension (Kindle vendor ext: `microsoft.com/WMDRMPD:10.1`) | NONE |

## Missing Device Property Codes (`DevicePropertyCode` enum)

mtp-rs defines PTP camera properties (0x5000 range) but none of the MTP-specific
device properties in the 0xD400 range.

| Hex    | MTP Spec Name              | Kindle Value                              | Notes                                          |
|--------|----------------------------|-------------------------------------------|-------------------------------------------------|
| 0xD401 | `SynchronizationPartner`   | `""` (empty, RW)                          | Standard MTP property                           |
| 0xD402 | `DeviceFriendlyName`       | `"Kindle Paperwhite GN433X..."` (RO)      | Already used implicitly via DeviceInfo           |
| 0xD404 | `SupportedFormatsOrdered`  | `Uint8(1)` (RO)                           | Boolean: device returns formats in priority order|
| 0xD405 | `DeviceIcon`               | ERROR: GeneralError                       | Kindle advertises but does not actually support  |
| 0xD407 | `PerceivedDeviceType`      | `Uint32(3)` (RO)                          | 3 = "Media Player" per MTP spec                 |

## Recommended Patch Order

### 1. `GetObjectPropsSupported` (0x9801) -- Highest impact

Enables dynamic property discovery per object format. Without it, consumers must
use a fixed list of known properties and tolerate errors.

Wire format: command takes one param (format code as `u32`), data phase returns a
`u16[]` array. Straightforward to add:
- Named variant in `OperationCode`
- New `PtpSession::get_object_props_supported(format: ObjectFormatCode) -> Result<Vec<ObjectPropertyCode>>`

### 2. `GetObjectPropDesc` (0x9802) -- Typed property parsing

Returns a property descriptor dataset (data type, default value, form/range) for one
object property code + format code. Similar structure to the existing `DevicePropDesc`.

Would need:
- Named variant in `OperationCode`
- New `ObjectPropDesc` type (mirroring `DevicePropDesc`)
- New `PtpSession::get_object_prop_desc(format, prop) -> Result<ObjectPropDesc>`

### 3. MTP device property codes (0xD401-0xD407)

Low effort: add named variants to `DevicePropertyCode` enum. No new session methods needed
since `get_device_prop_desc` / `get_device_prop_value` already work with any code.

### 4. `GetObjectPropList` (0x9805) -- Performance

Batch-fetches multiple properties for one or more objects in a single MTP round-trip.
Complex wire format (MTP "ObjectPropList" dataset with TLV-style entries).
Lower priority but significant performance win for inspector-style features.

### 5. Remaining operations

`SelfTest`, `SetObjectPropList`, `SendObjectPropList`, `GetObjectReferences`,
`SetObjectReferences`, WMDRMPD -- niche use cases, unlikely to be needed soon.

## Root Listing Performance (`list_objects_stream` quirk) -- **CRITICAL**

### Problem

When listing root (`parent=None`), `GetObjectHandles(parent=0)` on the Kindle
Paperwhite returns **all 2541 objects** on the storage — not just the ~20 root-level
items. mtp-rs then calls `GetObjectInfo` for each handle (one USB round-trip each)
and filters by `ParentFilter::Exact(ROOT)`, discarding ~2520 results.

The Android workaround (`parent=0xFFFFFFFF` instead of `0`) is gated behind
`is_android()`, which checks for `"android.com"` in `vendor_extension_desc`.
The Kindle (`"microsoft.com/WMDRMPD:10.1"`) doesn't match, so it takes the slow
path. The comment in the code already acknowledges this class of bug:

```
// Filter by exact parent (catches Fuji devices that return all objects for root)
```

…but the filter is applied *after* fetching `GetObjectInfo` for every handle.

### Current code (`Storage::list_objects_stream`)

```rust
let effective_parent = if parent.is_none() && self.inner.is_android() {
    Some(ObjectHandle::ALL)   // 0xFFFFFFFF — works, root-only
} else {
    parent                    // None → 0x00000000 — returns ALL objects on Kindle
};
```

### Diagnostic results

Tested on the Kindle Paperwhite with `examples/root_handle_test.rs`
(full output in `log/root_handle_test.log`):

| Call | Handles returned |
|------|-----------------|
| `GetObjectHandles(parent=0x00000000)` | **2541** (entire storage, flat dump) |
| `GetObjectHandles(parent=0xFFFFFFFF)` | **23** (root-level items only) |

All 23 handles from the `0xFFFFFFFF` call have `parent=0x00000000`, confirming they
are genuine root objects. The Kindle behaves identically to Android here — the
`is_android()` gate is the only thing preventing the fast path.

This is a **110x reduction** in `GetObjectInfo` round-trips (2541 → 23).

### Confirmed fix

Try `0xFFFFFFFF` first for all root listings, fall back to `0` + `ParentFilter`.
Remove the `is_android()` gate:

```rust
if parent.is_none() {
    let alt_result = self.inner.session
        .get_object_handles(self.id, None, Some(ObjectHandle::ALL))
        .await;
    match alt_result {
        Ok(handles) if !handles.is_empty() => {
            return Ok(ObjectListing {
                inner: Arc::clone(&self.inner),
                handles,
                cursor: 0,
                parent_filter: None, // device already filtered
            });
        }
        _ => {} // fall through to parent=0 path
    }
}
```

This covers Android, Kindle, Fuji, and any future device with the same quirk
without vendor-specific detection. One extra round-trip in the worst case (device
errors on `0xFFFFFFFF` → falls through to existing path).

### Kindle-specific evidence

| Fact | Value |
|------|-------|
| Vendor extension | `microsoft.com/WMDRMPD:10.1;microsoft.com/playready:1.10` |
| `is_android()` | `false` (no `"android.com"` in extension) |
| `GetObjectHandles(parent=0)` | 2541 handles (all objects on storage) |
| `GetObjectHandles(parent=0xFFFFFFFF)` | **23 handles (root only)** |
| Actual root items | 23 |

## How This Affects mtp-tui

The object inspector (`i` key) currently uses a fixed list of `ObjectPropertyCode`
variants and catches per-property errors. With patches 1 and 2 above:

- **Patch 1** would let us query the device for supported properties first, then
  only fetch those (no more try/fail on unsupported codes).
- **Patch 2** would let us parse raw property bytes using the declared data type
  instead of hardcoding type assumptions per property code.
- **Patch 4** would let us fetch all properties in one MTP command instead of N
  sequential `GetObjectPropValue` calls.

The root listing fix (above) is the highest-impact change for mtp-tui — it would
reduce the initial "Connecting to device" wait from ~30-60s to under 1s on the
Kindle Paperwhite.
