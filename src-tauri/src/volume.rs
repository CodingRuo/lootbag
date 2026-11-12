use std::{ffi::CString, fmt::Write, os::unix::ffi::OsStrExt, path::Path};

// SDK sys/attr.h: request only ATTR_VOL_INFO | ATTR_VOL_UUID, without opening file contents.
#[repr(C)]
struct AttributeList {
    bitmap_count: u16,
    reserved: u16,
    common: u32,
    volume: u32,
    directory: u32,
    file: u32,
    fork: u32,
}

#[repr(C)]
#[derive(Default)]
struct VolumeAttributes {
    length: u32,
    uuid: [u8; 16],
}

extern "C" {
    fn getattrlist(
        path: *const std::ffi::c_char,
        attributes: *mut AttributeList,
        buffer: *mut std::ffi::c_void,
        size: usize,
        options: std::ffi::c_uint,
    ) -> std::ffi::c_int;
}

pub(super) fn uuid(path: &Path) -> Result<String, String> {
    let path = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| "Ungültiger Pfad der externen Quelle")?;
    let mut attributes = AttributeList {
        bitmap_count: 5,
        reserved: 0,
        common: 0,
        volume: 0x8000_0000 | 0x0004_0000,
        directory: 0,
        file: 0,
        fork: 0,
    };
    let mut result = VolumeAttributes::default();
    // SAFETY: all pointers remain valid for this synchronous SDK call. The C layouts match
    // attrlist and the length-prefixed UUID response. FSOPT_NOFOLLOW does not follow a final
    // symlink; the shared library resolver separately rejects symbolic links.
    let status = unsafe {
        getattrlist(
            path.as_ptr(),
            &mut attributes,
            (&mut result as *mut VolumeAttributes).cast(),
            std::mem::size_of::<VolumeAttributes>(),
            1,
        )
    };
    if status != 0 {
        return Err(format!(
            "Laufwerksidentität nicht lesbar: {}",
            std::io::Error::last_os_error()
        ));
    }
    if result.length as usize != std::mem::size_of::<VolumeAttributes>() || result.uuid == [0; 16] {
        return Err("Dieses Laufwerk liefert keine stabile Volume-UUID".into());
    }
    let mut uuid = String::with_capacity(32);
    for byte in result.uuid {
        write!(uuid, "{byte:02x}").expect("writing to String cannot fail");
    }
    Ok(uuid)
}

// The same public volume attribute is read through a held FD for fingerprint ownership.
pub(super) fn uuid_from_file(file: &std::fs::File) -> Result<String, String> {
    use std::os::fd::AsRawFd;
    extern "C" {
        fn fgetattrlist(
            fd: i32,
            attributes: *mut AttributeList,
            buffer: *mut std::ffi::c_void,
            size: usize,
            options: std::ffi::c_uint,
        ) -> std::ffi::c_int;
    }
    let mut attributes = AttributeList {
        bitmap_count: 5,
        reserved: 0,
        common: 0,
        volume: 0x8000_0000 | 0x0004_0000,
        directory: 0,
        file: 0,
        fork: 0,
    };
    let mut result = VolumeAttributes::default();
    // SAFETY: held descriptor and SDK attrlist/length-prefixed UUID layout, synchronous call.
    let status = unsafe {
        fgetattrlist(
            file.as_raw_fd(),
            &mut attributes,
            (&mut result as *mut VolumeAttributes).cast(),
            std::mem::size_of::<VolumeAttributes>(),
            0,
        )
    };
    if status != 0
        || result.length as usize != std::mem::size_of::<VolumeAttributes>()
        || result.uuid == [0; 16]
    {
        return Err("Gehaltene Laufwerksidentität ist nicht verfügbar".into());
    }
    let mut uuid = String::with_capacity(32);
    for byte in result.uuid {
        write!(uuid, "{byte:02x}").expect("String write");
    }
    Ok(uuid)
}
