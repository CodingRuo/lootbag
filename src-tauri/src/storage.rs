use super::{Library, Result};
use std::{
    fs,
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

fn text(
    row: &rusqlite::Row<'_>,
    column: usize,
    bytes: &mut usize,
    cap: usize,
) -> Result<Option<String>> {
    let value = row.get_ref(column).map_err(|e| e.to_string())?;
    if value == rusqlite::types::ValueRef::Null {
        return Ok(None);
    }
    let value = value
        .as_str()
        .map_err(|_| "Ungültiger Text im Speicherkatalog")?;
    if value.len() > 4096 {
        return Err("Ein Katalogfeld überschreitet das Speicherbericht-Limit".into());
    }
    *bytes = bytes
        .checked_add(value.len())
        .filter(|bytes| *bytes <= cap)
        .ok_or("Katalogdaten überschreiten das Speicherbericht-Limit")?;
    Ok(Some(value.to_owned()))
}
pub(super) fn required_text(
    row: &rusqlite::Row<'_>,
    column: usize,
    bytes: &mut usize,
    cap: usize,
) -> Result<String> {
    text(row, column, bytes, cap)?.ok_or_else(|| "Katalogfeld fehlt".into())
}
pub(super) fn normal_path(value: &str) -> Result<PathBuf> {
    let path = PathBuf::from(value);
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err("Ungültiger Pfad im Speicherkatalog".into());
    }
    Ok(path)
}

pub(super) struct QueryBudget<'a> {
    connection: &'a rusqlite::Connection,
    _state: Box<(&'a AtomicBool, Instant, Duration)>,
}
impl<'a> QueryBudget<'a> {
    pub(super) fn install(
        connection: &'a rusqlite::Connection,
        cancelled: &'a AtomicBool,
        started: Instant,
        work: Duration,
    ) -> Self {
        let state = Box::new((cancelled, started, work));
        // SAFETY: a stable Box outlives synchronous SQLite statements; Drop unregisters before freeing it.
        unsafe {
            rusqlite::ffi::sqlite3_progress_handler(
                connection.handle(),
                1000,
                Some(query_budget),
                std::ptr::from_ref(state.as_ref()).cast_mut().cast(),
            );
        }
        Self {
            connection,
            _state: state,
        }
    }
}
unsafe extern "C" fn query_budget(pointer: *mut std::ffi::c_void) -> i32 {
    // SAFETY: installed pointer refers to the retained QueryBudget box on this worker; no callback can panic.
    let state = unsafe { &*pointer.cast::<(&AtomicBool, Instant, Duration)>() };
    i32::from(state.0.load(Ordering::Relaxed) || state.1.elapsed() >= state.2)
}
impl Drop for QueryBudget<'_> {
    fn drop(&mut self) {
        // SAFETY: the connection is still exclusively owned; clearing the callback releases the borrowed flag.
        unsafe {
            rusqlite::ffi::sqlite3_progress_handler(
                self.connection.handle(),
                0,
                None,
                std::ptr::null_mut(),
            );
        }
    }
}

#[cfg(target_os = "macos")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Stamp {
    device: u64,
    inode: u64,
    pub(in crate::library) size: u64,
    modified: (i64, i64),
    changed: (i64, i64),
    links: u64,
    mode: u32,
}
#[cfg(target_os = "macos")]
impl Stamp {
    pub(super) fn read(metadata: &fs::Metadata) -> Self {
        use std::os::unix::fs::MetadataExt;
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            size: metadata.len(),
            modified: (metadata.mtime(), metadata.mtime_nsec()),
            changed: (metadata.ctime(), metadata.ctime_nsec()),
            links: metadata.nlink(),
            mode: metadata.mode(),
        }
    }
    pub(super) fn is_file(&self) -> bool {
        self.mode & 0xf000 == 0x8000
    }
    pub(super) fn identity(&self) -> (u64, u64) {
        (self.device, self.inode)
    }
}
#[cfg(target_os = "macos")]
pub(super) mod anchored {
    use super::*;
    use std::{
        ffi::{CString, OsStr},
        os::{
            fd::{AsRawFd, FromRawFd},
            unix::ffi::OsStrExt,
        },
    };
    extern "C" {
        fn openat(fd: i32, name: *const std::ffi::c_char, flags: i32, ...) -> i32;
        #[cfg_attr(target_arch = "x86_64", link_name = "fstatat$INODE64")]
        fn fstatat(fd: i32, name: *const std::ffi::c_char, stat: *mut Stat, flags: i32) -> i32;
    }
    // Darwin SDK sys/stat.h 64-bit inode layout, for the two supported Mac architectures.
    #[repr(C)]
    struct Stat {
        device: i32,
        mode: u16,
        links: u16,
        inode: u64,
        uid: u32,
        gid: u32,
        rdev: i32,
        accessed: [i64; 2],
        modified: [i64; 2],
        changed: [i64; 2],
        born: [i64; 2],
        size: i64,
        blocks: i64,
        block_size: i32,
        flags: u32,
        generation: u32,
        spare: i32,
        quad_spare: [i64; 2],
    }
    fn name(name: &OsStr) -> Result<CString> {
        if name.as_bytes().len() > 1023
            || name.as_bytes().is_empty()
            || name.as_bytes().contains(&b'/')
        {
            return Err("Ungültiger Dateiname".into());
        }
        CString::new(name.as_bytes()).map_err(|_| "Ungültiger Dateiname".into())
    }
    // Metadata-only lookups must not open/close an inode that SQLite may have locked:
    // POSIX record locks are process-owned and any close of that inode releases them.
    pub(in crate::library) fn optional_child_stamp(
        parent: &fs::File,
        leaf: &OsStr,
    ) -> Result<Option<Stamp>> {
        let leaf = name(leaf)?;
        let mut stat = std::mem::MaybeUninit::<Stat>::uninit();
        // SAFETY: qualified SDK layout, live held parent and bounded single leaf;
        // AT_SYMLINK_NOFOLLOW avoids following a replacement link and acquires no descriptor.
        if unsafe { fstatat(parent.as_raw_fd(), leaf.as_ptr(), stat.as_mut_ptr(), 0x20) } != 0 {
            let error = std::io::Error::last_os_error();
            return if error.kind() == std::io::ErrorKind::NotFound {
                Ok(None)
            } else {
                Err(error.to_string())
            };
        }
        let stat = unsafe { stat.assume_init() };
        if stat.mode & 0xf000 == 0xa000 {
            return Err("Symbolischer Link ist nicht zulässig".into());
        }
        Ok(Some(Stamp {
            device: stat.device as u64,
            inode: stat.inode,
            size: stat.size as u64,
            modified: (stat.modified[0], stat.modified[1]),
            changed: (stat.changed[0], stat.changed[1]),
            links: stat.links as u64,
            mode: stat.mode as u32,
        }))
    }
    pub(in crate::library) fn child_stamp(parent: &fs::File, leaf: &OsStr) -> Result<Stamp> {
        optional_child_stamp(parent, leaf)?.ok_or_else(|| "Datei wurde entfernt".into())
    }
    const FLAGS: i32 = 4 | 0x100 | 0x1000000; // Darwin O_NONBLOCK | O_NOFOLLOW | O_CLOEXEC.
    pub(in crate::library) fn child(parent: &fs::File, leaf: &OsStr) -> Result<fs::File> {
        open_child(parent, leaf, FLAGS)
    }
    pub(in crate::library) fn directory(parent: &fs::File, leaf: &OsStr) -> Result<fs::File> {
        open_child(parent, leaf, FLAGS | 0x100000) // O_DIRECTORY: a replaced regular inode is never opened.
    }
    fn open_child(parent: &fs::File, leaf: &OsStr, flags: i32) -> Result<fs::File> {
        let name = name(leaf)?;
        // SAFETY: live owned parent descriptor, bounded single NUL-terminated leaf; no symlinks or blocking FIFO open.
        let fd = unsafe { openat(parent.as_raw_fd(), name.as_ptr(), flags) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        // SAFETY: successful openat transfers one descriptor.
        Ok(unsafe { fs::File::from_raw_fd(fd) })
    }
    pub(super) fn journal(parent: &fs::File) -> Result<Option<fs::File>> {
        // SAFETY: fixed leaf beneath a held directory; no links or blocking special-file open.
        let fd = unsafe {
            openat(
                parent.as_raw_fd(),
                c"catalog.sqlite3-journal".as_ptr(),
                FLAGS,
            )
        };
        if fd < 0 {
            let error = std::io::Error::last_os_error();
            return if error.kind() == std::io::ErrorKind::NotFound {
                Ok(None)
            } else {
                Err(error.to_string())
            };
        }
        // SAFETY: successful openat transfers one descriptor.
        Ok(Some(unsafe { fs::File::from_raw_fd(fd) }))
    }
    pub(in crate::library) fn root(path: &Path) -> Result<fs::File> {
        if path.as_os_str().as_encoded_bytes().len() > 4096 {
            return Err("Bibliothekspfad überschreitet das Messlimit".into());
        }
        if !path.is_absolute() {
            return Err("Bibliothekswurzel ist nicht absolut".into());
        }
        let mut file = fs::File::open("/").map_err(|e| e.to_string())?;
        for part in path.components() {
            if let Component::Normal(name) = part {
                file = directory(&file, name)?;
                if !file.metadata().map_err(|e| e.to_string())?.is_dir() {
                    return Err("Bibliotheksordner wurde ersetzt".into());
                }
            } else if !matches!(part, Component::RootDir) {
                return Err("Ungültige Bibliothekswurzel".into());
            }
        }
        Ok(file)
    }
}

#[cfg(target_os = "macos")]
fn rollback_journal_stamp(journal: &fs::File) -> Result<Stamp> {
    use std::os::unix::fs::FileExt;
    let metadata = journal.metadata().map_err(|e| e.to_string())?;
    if !metadata.is_file() {
        return Err("Rollback-Journal ist keine reguläre Datei".into());
    }
    let stamp = Stamp::read(&metadata);
    if metadata.len() > 512 {
        let mut header = [0_u8; 8];
        journal
            .read_exact_at(&mut header, 0)
            .map_err(|e| e.to_string())?;
        if header.iter().any(|byte| *byte != 0) {
            return Err("Rollback-Journal benötigt möglicherweise Wiederherstellung; Speichermessung ist ohne Änderungen nicht verfügbar".into());
        }
    }
    if Stamp::read(&journal.metadata().map_err(|e| e.to_string())?) != stamp {
        return Err("Rollback-Journal wurde beim Prüfen verändert".into());
    }
    Ok(stamp)
}

#[cfg(target_os = "macos")]
fn immutable_catalog(root: &Path, catalog: &fs::File) -> Result<Library> {
    use std::os::fd::AsRawFd;
    // A held descriptor fixes the inode; immutable URI mode suppresses SQLite locking and sidecar writes.
    let uri = format!("file:/dev/fd/{}?immutable=1", catalog.as_raw_fd());
    let connection = rusqlite::Connection::open_with_flags(
        uri,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
    )
    .map_err(|e| format!("Katalog kann nicht unverändernd geöffnet werden: {e}"))?;
    // SAFETY: this private connection is exclusively owned and has no prepared statements yet.
    unsafe {
        rusqlite::ffi::sqlite3_limit(
            connection.handle(),
            rusqlite::ffi::SQLITE_LIMIT_LENGTH,
            32_768,
        );
        rusqlite::ffi::sqlite3_limit(
            connection.handle(),
            rusqlite::ffi::SQLITE_LIMIT_SQL_LENGTH,
            32_768,
        );
    }
    connection
        .pragma_update(None, "query_only", true)
        .map_err(|e| e.to_string())?;
    let version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(|e| e.to_string())?;
    if version != 1 {
        return Err("Katalogversion ist für die Speichermessung nicht verfügbar".into());
    }
    connection.prepare("SELECT a.id FROM active_assets a LEFT JOIN folders f ON 0 LEFT JOIN folder_assets fa ON 0 LIMIT 0").map_err(|_| "Bitte die Bibliothek zuerst in der App aktualisieren")?;
    Ok(Library {
        root: root.to_owned(),
        connection,
    })
}

// Private readers keep this lease until their SQLite and raw descriptors close.
// Promotion is atomic and fails busy rather than copying while a cancelled reader
// can still close the same inode and release POSIX SQLite record locks.
#[cfg(target_os = "macos")]
#[derive(Default)]
struct CatalogUse {
    readers: usize,
    publishing: bool,
}
#[cfg(target_os = "macos")]
static CATALOG_USERS: std::sync::Mutex<std::collections::BTreeMap<(u64, u64), CatalogUse>> =
    std::sync::Mutex::new(std::collections::BTreeMap::new());
#[cfg(target_os = "macos")]
pub(super) struct CatalogLease {
    identity: (u64, u64),
    publishing: bool,
}
#[cfg(target_os = "macos")]
impl CatalogLease {
    fn read(identity: (u64, u64)) -> Result<Self> {
        let mut users = CATALOG_USERS
            .lock()
            .map_err(|_| "Katalogprüfung ist beschäftigt")?;
        let entry = users.entry(identity).or_default();
        if entry.publishing {
            return Err("Katalogprüfung ist beschäftigt; bitte erneut vorbereiten".into());
        }
        entry.readers += 1;
        Ok(Self {
            identity,
            publishing: false,
        })
    }
}
#[cfg(target_os = "macos")]
pub(super) fn reserve_catalog_publication(directory: &fs::File) -> Result<CatalogLease> {
    let stamp = anchored::child_stamp(directory, std::ffi::OsStr::new("catalog.sqlite3"))?;
    if !stamp.is_file() {
        return Err("Katalog ist keine reguläre Datei".into());
    }
    let identity = stamp.identity();
    let mut users = CATALOG_USERS
        .lock()
        .map_err(|_| "Katalogprüfung ist beschäftigt")?;
    let entry = users.entry(identity).or_default();
    if entry.readers != 0 || entry.publishing {
        return Err(
            "Eine Katalogprüfung läuft noch; bitte nach ihrem Ende erneut vorbereiten".into(),
        );
    }
    entry.readers = 1;
    entry.publishing = true;
    Ok(CatalogLease {
        identity,
        publishing: true,
    })
}
#[cfg(target_os = "macos")]
impl Drop for CatalogLease {
    fn drop(&mut self) {
        if let Ok(mut users) = CATALOG_USERS.lock() {
            if let Some(entry) = users.get_mut(&self.identity) {
                entry.readers -= 1;
                if self.publishing {
                    entry.publishing = false;
                }
                if entry.readers == 0 {
                    users.remove(&self.identity);
                }
            }
        }
    }
}
#[cfg(target_os = "macos")]
#[derive(Debug)]
pub(super) enum ReservedOpenError {
    Busy(String),
    Unavailable(String),
}
// Both private read-only workflows retain the same descriptor/journal qualification.
#[cfg(target_os = "macos")]
pub(super) struct CatalogReader {
    pub(super) library: Library,
    pub(super) directory: fs::File,
    catalog: fs::File,
    root_stamp: Stamp,
    catalog_stamp: Stamp,
    journal_stamp: Option<Stamp>,
    journal: Option<fs::File>,
    // Field destruction order is significant: release only after every catalogue descriptor.
    _lease: CatalogLease,
}
#[cfg(target_os = "macos")]
impl CatalogReader {
    pub(super) fn open(root: &Path) -> Result<Self> {
        let directory = anchored::root(root)?;
        let expected = anchored::child_stamp(&directory, std::ffi::OsStr::new("catalog.sqlite3"))?;
        let lease = CatalogLease::read(expected.identity())?;
        Self::open_owned(root, directory, expected, lease)
    }
    // Publication reserves before raw catalogue descriptors, so cancelled readers cannot
    // close the same inode during its normal SQLite transaction.
    pub(super) fn open_reserved(root: &Path) -> std::result::Result<Self, ReservedOpenError> {
        let directory = anchored::root(root).map_err(ReservedOpenError::Unavailable)?;
        let expected = anchored::child_stamp(&directory, std::ffi::OsStr::new("catalog.sqlite3"))
            .map_err(ReservedOpenError::Unavailable)?;
        let lease = reserve_catalog_publication(&directory).map_err(ReservedOpenError::Busy)?;
        Self::open_owned(root, directory, expected, lease).map_err(ReservedOpenError::Unavailable)
    }
    fn open_owned(
        root: &Path,
        directory: fs::File,
        expected_catalog: Stamp,
        lease: CatalogLease,
    ) -> Result<Self> {
        use std::io::Read;
        let root_stamp = Stamp::read(&directory.metadata().map_err(|e| e.to_string())?);
        let mut catalog = anchored::child(&directory, std::ffi::OsStr::new("catalog.sqlite3"))?;
        if !catalog.metadata().map_err(|e| e.to_string())?.is_file() {
            return Err("Katalog ist keine reguläre Datei".into());
        }
        let catalog_stamp = Stamp::read(&catalog.metadata().map_err(|e| e.to_string())?);
        if catalog_stamp != expected_catalog {
            return Err("Katalog wurde beim Öffnen ersetzt".into());
        }
        let mut header = [0_u8; 100];
        catalog
            .read_exact(&mut header)
            .map_err(|_| "Katalogkopf ist unvollständig")?;
        if &header[..16] != b"SQLite format 3\0" || header[18] != 1 || header[19] != 1 {
            return Err("Nur SQLite-Bibliotheken mit Rollback-Journal sind für unverändernde Prüfungen qualifiziert".into());
        }
        let journal = anchored::journal(&directory)?;
        let journal_stamp = journal.as_ref().map(rollback_journal_stamp).transpose()?;
        let library = immutable_catalog(root, &catalog)?;
        let reader = Self {
            library,
            directory,
            catalog,
            root_stamp,
            catalog_stamp,
            journal_stamp,
            journal,
            _lease: lease,
        };
        reader.validate()?;
        Ok(reader)
    }
    pub(super) fn catalog_file(&self) -> &fs::File {
        &self.catalog
    }
    pub(super) fn validate(&self) -> Result<()> {
        self.validate_with_root(self.root_stamp)
    }
    // Only the guarded importer supplies a separately proven owned-stage root transition.
    pub(super) fn validate_with_root(&self, expected_root: Stamp) -> Result<()> {
        if self
            .journal
            .as_ref()
            .map(rollback_journal_stamp)
            .transpose()
            .ok()
            != Some(self.journal_stamp)
            || anchored::optional_child_stamp(
                &self.directory,
                std::ffi::OsStr::new("catalog.sqlite3-journal"),
            )
            .ok()
                != Some(self.journal_stamp)
        {
            return Err("Rollback-Journal wurde verändert oder benötigt Wiederherstellung; bitte die Bibliothek zuerst öffnen".into());
        }
        if anchored::root(&self.library.root)
            .and_then(|file| file.metadata().map_err(|e| e.to_string()))
            .map(|m| Stamp::read(&m))
            .ok()
            != Some(expected_root)
        {
            return Err("Bibliotheksordner wurde verändert oder ersetzt".into());
        }
        if Stamp::read(&self.catalog.metadata().map_err(|e| e.to_string())?) != self.catalog_stamp
            || anchored::child_stamp(&self.directory, std::ffi::OsStr::new("catalog.sqlite3")).ok()
                != Some(self.catalog_stamp)
        {
            return Err("Katalogdatei wurde verändert oder ersetzt; bitte aktualisieren".into());
        }
        Ok(())
    }
}
