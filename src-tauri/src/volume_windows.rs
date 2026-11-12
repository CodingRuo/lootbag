// Windows counterpart of volume.rs. std's file_index()/volume_serial_number() are still
// unstable (windows_by_handle), so this asks kernel32 directly; no extra crate is needed.
use std::{
    ffi::c_void,
    os::windows::{ffi::OsStrExt, fs::OpenOptionsExt, io::AsRawHandle},
    path::Path,
};

#[repr(C)]
#[derive(Default)]
struct ByHandleFileInformation {
    attributes: u32,
    created: [u32; 2],
    accessed: [u32; 2],
    written: [u32; 2],
    volume_serial: u32,
    size_high: u32,
    size_low: u32,
    links: u32,
    index_high: u32,
    index_low: u32,
}

#[link(name = "kernel32")]
extern "system" {
    fn GetFileInformationByHandle(file: *mut c_void, info: *mut ByHandleFileInformation) -> i32;
    fn GetDiskFreeSpaceExW(
        directory: *const u16,
        available: *mut u64,
        total: *mut u64,
        free: *mut u64,
    ) -> i32;
}

/// Volume serial number plus NTFS file index; stable across renames and moves on one volume.
// shortcut: ReFS uses 128-bit file IDs, of which this reads the legacy 64 bits; upgrade to
// GetFileInformationByHandleEx(FileIdInfo) if ReFS sources show identity collisions.
pub(super) fn identity(path: &Path) -> Result<String, String> {
    let file = std::fs::OpenOptions::new()
        .access_mode(0) // Metadata only: no read access to file contents is requested.
        // FILE_FLAG_BACKUP_SEMANTICS opens directories; FILE_FLAG_OPEN_REPARSE_POINT does not follow links.
        .custom_flags(0x0200_0000 | 0x0020_0000)
        .open(path)
        .map_err(|error| format!("Dateiidentität nicht lesbar: {error}"))?;
    let mut info = ByHandleFileInformation::default();
    // SAFETY: the handle stays open for this synchronous call and `info` matches the SDK layout.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
        return Err(format!(
            "Dateiidentität nicht lesbar: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(format!(
        "win-volume:{:08x}:{}",
        info.volume_serial,
        (u64::from(info.index_high) << 32) | u64::from(info.index_low)
    ))
}

pub(super) fn available_space(path: &Path) -> Result<u64, String> {
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    let mut available = 0;
    // SAFETY: `wide` is NUL-terminated and the out pointers are valid for this call.
    let ok = unsafe {
        GetDiskFreeSpaceExW(
            wide.as_ptr(),
            &mut available,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Err(format!(
            "Freier Speicherplatz kann nicht ermittelt werden: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(available)
}
