use mtp_rs::ptp::{ObjectFormatCode, ObjectPropertyCode};

use crate::ui::format_size;

// TODO(mtp-rs 0x9801): replace with GetObjectPropsSupported to discover properties
// dynamically per format instead of hardcoding. See MTP_RS_GAPS.md patch #1.
pub const INSPECTOR_PROPERTIES: &[ObjectPropertyCode] = &[
    ObjectPropertyCode::StorageId,
    ObjectPropertyCode::ObjectFormat,
    ObjectPropertyCode::ProtectionStatus,
    ObjectPropertyCode::ObjectSize,
    ObjectPropertyCode::ObjectFileName,
    ObjectPropertyCode::DateCreated,
    ObjectPropertyCode::DateModified,
    ObjectPropertyCode::ParentObject,
    ObjectPropertyCode::Name,
];

pub fn prop_name(code: ObjectPropertyCode) -> String {
    match code {
        ObjectPropertyCode::StorageId => "StorageId".into(),
        ObjectPropertyCode::ObjectFormat => "ObjectFormat".into(),
        ObjectPropertyCode::ProtectionStatus => "ProtectionStatus".into(),
        ObjectPropertyCode::ObjectSize => "ObjectSize".into(),
        ObjectPropertyCode::ObjectFileName => "ObjectFileName".into(),
        ObjectPropertyCode::DateCreated => "DateCreated".into(),
        ObjectPropertyCode::DateModified => "DateModified".into(),
        ObjectPropertyCode::ParentObject => "ParentObject".into(),
        ObjectPropertyCode::Name => "Name".into(),
        ObjectPropertyCode::Unknown(c) => format!("0x{c:04X}"),
    }
}

// TODO(mtp-rs 0x9802): use GetObjectPropDesc to get the declared data type per
// property instead of hardcoding the type map here. See MTP_RS_GAPS.md patch #2.
pub fn decode_prop_value(code: ObjectPropertyCode, bytes: &[u8]) -> String {
    use mtp_rs::ptp::{unpack_string, unpack_u16, unpack_u32, unpack_u64};

    match code {
        ObjectPropertyCode::ObjectSize => unpack_u64(bytes)
            .map(|v| format!("{} ({v} bytes)", format_size(v)))
            .unwrap_or_else(|_| hex_dump(bytes)),
        ObjectPropertyCode::ObjectFormat | ObjectPropertyCode::ProtectionStatus => {
            unpack_u16(bytes)
                .map(|v| format!("0x{v:04X}"))
                .unwrap_or_else(|_| hex_dump(bytes))
        }
        ObjectPropertyCode::StorageId | ObjectPropertyCode::ParentObject => unpack_u32(bytes)
            .map(|v| format!("0x{v:08X}"))
            .unwrap_or_else(|_| hex_dump(bytes)),
        ObjectPropertyCode::ObjectFileName
        | ObjectPropertyCode::DateCreated
        | ObjectPropertyCode::DateModified
        | ObjectPropertyCode::Name => unpack_string(bytes)
            .map(|(s, _)| if s.is_empty() { "(empty)".into() } else { s })
            .unwrap_or_else(|_| hex_dump(bytes)),
        ObjectPropertyCode::Unknown(_) => hex_dump(bytes),
    }
}

fn hex_dump(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return "(empty)".into();
    }
    let display: Vec<String> = bytes.iter().take(32).map(|b| format!("{b:02X}")).collect();
    let suffix = if bytes.len() > 32 {
        format!("... ({} bytes total)", bytes.len())
    } else {
        String::new()
    };
    format!("{}{suffix}", display.join(" "))
}

pub fn format_object_format(code: ObjectFormatCode) -> String {
    match code {
        ObjectFormatCode::Undefined => "Undefined (0x3000)".into(),
        ObjectFormatCode::Association => "Association/Folder (0x3001)".into(),
        ObjectFormatCode::Text => "Text (0x3004)".into(),
        ObjectFormatCode::Html => "HTML (0x3005)".into(),
        ObjectFormatCode::Jpeg => "JPEG (0x3801)".into(),
        ObjectFormatCode::Png => "PNG (0x380B)".into(),
        ObjectFormatCode::Gif => "GIF (0x3807)".into(),
        ObjectFormatCode::Tiff => "TIFF (0x3804)".into(),
        ObjectFormatCode::Bmp => "BMP (0x3808)".into(),
        ObjectFormatCode::Mp3 => "MP3 (0x3009)".into(),
        ObjectFormatCode::Wav => "WAV (0x3008)".into(),
        ObjectFormatCode::Avi => "AVI (0x300A)".into(),
        ObjectFormatCode::Mpeg => "MPEG (0x300B)".into(),
        ObjectFormatCode::Mp4Container => "MP4 (0xB982)".into(),
        ObjectFormatCode::M4aAudio => "M4A (0xB984)".into(),
        ObjectFormatCode::WmaAudio => "WMA (0xB901)".into(),
        ObjectFormatCode::WmvVideo => "WMV (0xB981)".into(),
        ObjectFormatCode::FlacAudio => "FLAC (0xB906)".into(),
        ObjectFormatCode::Unknown(c) => format!("Unknown(0x{c:04X})"),
        other => format!("{other:?}"),
    }
}

pub fn format_datetime(dt: &mtp_rs::ptp::DateTime) -> String {
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        dt.year, dt.month, dt.day, dt.hour, dt.minute, dt.second
    )
}
