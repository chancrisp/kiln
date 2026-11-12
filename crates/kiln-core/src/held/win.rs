//! The Windows calls the holding roots and cross-volume moves need: the
//! volume a path is on, the kind of drive it is, the drives mounted, the
//! hidden attribute, `CopyFileExW` with a progress routine, a rename that
//! never replaces, a file's ID, the user's SID, and a folder only that
//! user can open.

use std::ffi::{c_void, OsStr, OsString};
use std::io;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::os::windows::fs::MetadataExt;
use std::os::windows::io::AsRawHandle;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};

use windows_sys::Win32::Foundation::{
    CloseHandle, LocalFree, ERROR_REQUEST_ABORTED, HANDLE, HLOCAL,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{
    GetTokenInformation, TokenUser, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY,
    TOKEN_USER,
};
use windows_sys::Win32::Storage::FileSystem::{
    CopyFileExW, CreateDirectoryW, GetDriveTypeW, GetFileInformationByHandle, GetLogicalDrives,
    GetVolumePathNameW, MoveFileExW, SetFileAttributesW, BY_HANDLE_FILE_INFORMATION,
    FILE_ATTRIBUTE_HIDDEN, PROGRESS_CANCEL, PROGRESS_CONTINUE,
};
use windows_sys::Win32::System::Diagnostics::Debug::{SetThreadErrorMode, SEM_FAILCRITICALERRORS};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

use super::roots::{is_sid, DriveKind};
use crate::converter::CancelToken;

// `GetDriveTypeW`'s answers (WindowsProgramming; named here to keep the
// feature list short).
const DRIVE_REMOVABLE: u32 = 2;
const DRIVE_FIXED: u32 = 3;
const DRIVE_REMOTE: u32 = 4;

/// Paths this long or longer get the `\\?\` prefix, so calls that would
/// stop at `MAX_PATH` (260) take them whole.
const LONG: usize = 240;

/// `path` as a NUL-terminated wide string, with the `\\?\` prefix when it
/// is long and absolute.
pub(super) fn wide(path: &Path) -> Vec<u16> {
    let text = path.as_os_str();
    let mut out: Vec<u16> = text.encode_wide().collect();
    if out.len() >= LONG {
        if let Some(s) = text.to_str() {
            let s = s.replace('/', "\\");
            let long = if s.starts_with(r"\\?\") || s.starts_with(r"\\.\") {
                None
            } else if let Some(unc) = s.strip_prefix(r"\\") {
                Some(format!(r"\\?\UNC\{unc}"))
            } else if s.as_bytes().get(1) == Some(&b':') && s.as_bytes().get(2) == Some(&b'\\') {
                Some(format!(r"\\?\{s}"))
            } else {
                None
            };
            if let Some(long) = long {
                out = OsStr::new(&long).encode_wide().collect();
            }
        }
    }
    out.push(0);
    out
}

/// `\\?\C:\x` as `C:\x`, and `\\?\UNC\s\x` as `\\s\x`: the prefix [`wide`]
/// may add, taken back off.
fn plain(text: String) -> String {
    if let Some(unc) = text.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{unc}")
    } else if let Some(rest) = text.strip_prefix(r"\\?\") {
        let b = rest.as_bytes();
        if b.get(1) == Some(&b':') {
            rest.to_owned()
        } else {
            text
        }
    } else {
        text
    }
}

/// Windows doesn't pop up "There is no disk in the drive" on this thread
/// while this lives: a card reader with no card fails quietly instead.
pub(super) struct QuietDrives(u32);

impl QuietDrives {
    pub(super) fn new() -> QuietDrives {
        let mut old = 0u32;
        // SAFETY: `old` is a valid place for the previous mode to be
        // written. The call changes only this thread's error mode.
        unsafe { SetThreadErrorMode(SEM_FAILCRITICALERRORS, &mut old) };
        QuietDrives(old)
    }
}

impl Drop for QuietDrives {
    fn drop(&mut self) {
        // SAFETY: restores the mode `new` saved; null is allowed for the
        // previous mode.
        unsafe { SetThreadErrorMode(self.0, std::ptr::null_mut()) };
    }
}

/// The root of the volume `path` is on, as `GetVolumePathNameW` gives it:
/// `E:\`, a mounted folder such as `C:\mnt\disk\`, or `\\server\share\`.
/// A path that doesn't exist yet is answered for its nearest existing
/// folder.
pub(super) fn volume_root(path: &Path) -> Option<PathBuf> {
    let _quiet = QuietDrives::new();
    let start = path
        .ancestors()
        .find(|p| !p.as_os_str().is_empty() && p.exists())
        .unwrap_or(path);
    let name = wide(start);
    let mut buf = vec![0u16; name.len().max(MAX_PATH_BUF)];
    let len = u32::try_from(buf.len()).ok()?;
    // SAFETY: `name` is NUL-terminated, and `buf` is writable for `len`
    // wide characters, the length passed.
    let ok = unsafe { GetVolumePathNameW(name.as_ptr(), buf.as_mut_ptr(), len) };
    if ok == 0 {
        return None;
    }
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    let mut text = OsString::from_wide(&buf[..end]).into_string().ok()?;
    if text.is_empty() {
        return None;
    }
    if !text.ends_with('\\') {
        text.push('\\');
    }
    Some(PathBuf::from(plain(text)))
}

/// Room for a volume path: `MAX_PATH` and its NUL.
const MAX_PATH_BUF: usize = 261;

/// What kind of drive the volume at `root` is.
pub(super) fn drive_kind(root: &Path) -> DriveKind {
    let _quiet = QuietDrives::new();
    let mut name = root.as_os_str().to_owned();
    if !root.as_os_str().to_string_lossy().ends_with('\\') {
        name.push("\\");
    }
    let name = wide(Path::new(&name));
    // SAFETY: `name` is NUL-terminated.
    match unsafe { GetDriveTypeW(name.as_ptr()) } {
        DRIVE_FIXED => DriveKind::Fixed,
        DRIVE_REMOVABLE => DriveKind::Removable,
        DRIVE_REMOTE => DriveKind::Network,
        _ => DriveKind::Other,
    }
}

/// `A:\` to `Z:\` for every drive letter mounted now.
pub(super) fn logical_drives() -> Vec<PathBuf> {
    // SAFETY: takes nothing, returns a bit mask.
    let mask = unsafe { GetLogicalDrives() };
    (0..26u8)
        .filter(|i| mask & (1 << i) != 0)
        .map(|i| PathBuf::from(format!("{}:\\", char::from(b'A' + i))))
        .collect()
}

/// Mark `dir` hidden, keeping its other attributes.
pub(super) fn set_hidden(dir: &Path) -> io::Result<()> {
    let attrs = std::fs::symlink_metadata(dir)?.file_attributes();
    if attrs & FILE_ATTRIBUTE_HIDDEN != 0 {
        return Ok(());
    }
    let name = wide(dir);
    // SAFETY: `name` is NUL-terminated.
    let ok = unsafe { SetFileAttributesW(name.as_ptr(), attrs | FILE_ATTRIBUTE_HIDDEN) };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Whether `dir` has the hidden attribute.
#[cfg(test)]
pub(super) fn is_hidden(dir: &Path) -> bool {
    std::fs::symlink_metadata(dir).is_ok_and(|m| m.file_attributes() & FILE_ATTRIBUTE_HIDDEN != 0)
}

/// The ID of the open file `f`: its volume's serial number and its file
/// index, the same for every name and link that leads to the one file.
pub(super) fn file_id(f: &std::fs::File) -> io::Result<(u32, u64)> {
    // SAFETY: an all-zero BY_HANDLE_FILE_INFORMATION is a valid value.
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: the handle is open for as long as `f` is, and `info` is a
    // valid place to write.
    let ok = unsafe { GetFileInformationByHandle(f.as_raw_handle() as HANDLE, &mut info) };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    let index = (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow);
    Ok((info.dwVolumeSerialNumber, index))
}

/// Rename `from` to `to` only if nothing is at `to`: `MoveFileExW` without
/// `MOVEFILE_REPLACE_EXISTING` or `MOVEFILE_COPY_ALLOWED`. A name that is
/// taken fails with `ErrorKind::AlreadyExists`, and another volume with
/// `ERROR_NOT_SAME_DEVICE`; neither touches either file.
pub(super) fn rename_new(from: &Path, to: &Path) -> io::Result<()> {
    let (src, dst) = (wide(from), wide(to));
    // SAFETY: both names are NUL-terminated. Flags 0: never replaces, and
    // never copies.
    let ok = unsafe { MoveFileExW(src.as_ptr(), dst.as_ptr(), 0) };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Memory Windows handed back that must go with `LocalFree`.
struct Local(HLOCAL);

impl Drop for Local {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: the pointer came from a Windows call that says to
            // free it with `LocalFree`, and is freed once, here.
            unsafe { LocalFree(self.0) };
        }
    }
}

/// The string form of a NUL-terminated wide string Windows wrote.
///
/// # Safety
/// `p` must point to a NUL-terminated wide string.
unsafe fn from_wide_ptr(p: *const u16) -> Option<String> {
    let mut len = 0;
    // SAFETY: the caller promises a NUL ends the string, so every read up
    // to it is inside it.
    while unsafe { *p.add(len) } != 0 {
        len += 1;
    }
    // SAFETY: `len` characters from `p` were just read.
    let chars = unsafe { std::slice::from_raw_parts(p, len) };
    String::from_utf16(chars).ok()
}

/// The SID of the user this process runs as, in its string form
/// (`S-1-5-21-...`).
pub(super) fn user_sid() -> Option<String> {
    let mut token: HANDLE = std::ptr::null_mut();
    // SAFETY: `GetCurrentProcess` returns a pseudo-handle that needs no
    // closing; `token` is a valid place for the token handle.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return None;
    }
    let sid = (|| {
        let mut size = 0u32;
        // SAFETY: a size query: no buffer, length 0, `size` written.
        unsafe { GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut size) };
        if size == 0 {
            return None;
        }
        // u64s, so the TOKEN_USER at its start is aligned.
        let mut buf = vec![0u64; (size as usize).div_ceil(8)];
        // SAFETY: `buf` is writable for `size` bytes (at least), the
        // length passed.
        let ok = unsafe {
            GetTokenInformation(token, TokenUser, buf.as_mut_ptr().cast(), size, &mut size)
        };
        if ok == 0 {
            return None;
        }
        // SAFETY: Windows filled `buf` with a TOKEN_USER (aligned, as `buf`
        // is of u64s); its SID points inside `buf`, which outlives this.
        let user = unsafe { &*(buf.as_ptr() as *const TOKEN_USER) };
        let mut text: *mut u16 = std::ptr::null_mut();
        // SAFETY: the SID is valid while `buf` lives; `text` gets a string
        // to free with `LocalFree`.
        if unsafe { ConvertSidToStringSidW(user.User.Sid, &mut text) } == 0 {
            return None;
        }
        let _free = Local(text.cast());
        // SAFETY: `ConvertSidToStringSidW` wrote a NUL-terminated string.
        unsafe { from_wide_ptr(text) }
    })();
    // SAFETY: `token` was opened above and is closed once.
    unsafe { CloseHandle(token) };
    sid.filter(|s| is_sid(s))
}

/// Make the folder `dir` with a protected DACL: full control for the user
/// `sid`, SYSTEM and Administrators, and nothing inherited. On a drive
/// with no permissions (FAT, exFAT) it is a plain folder. Fails with
/// `AlreadyExists` if something is there.
pub(super) fn create_private_dir(dir: &Path, sid: &str) -> io::Result<()> {
    if !is_sid(sid) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "not a SID"));
    }
    // O: the user owns it, even from an elevated token (whose default
    // owner is Administrators), so [`is_users_own`] can tell it from one
    // another account made. P: protected, so nothing is inherited from
    // `.kiln-held`. OICI: what Kiln makes inside gets the same.
    let sddl = format!("O:{sid}D:P(A;OICI;FA;;;{sid})(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)");
    let sddl: Vec<u16> = OsStr::new(&sddl).encode_wide().chain([0]).collect();
    let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    // SAFETY: `sddl` is NUL-terminated; `sd` gets a descriptor to free
    // with `LocalFree`; the size isn't wanted (null).
    let ok = unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut sd,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    let _free = Local(sd);
    let attrs = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: sd,
        bInheritHandle: 0,
    };
    let name = wide(dir);
    // SAFETY: `name` is NUL-terminated; `attrs` and the descriptor it
    // points to live until the call returns.
    if unsafe { CreateDirectoryW(name.as_ptr(), &attrs) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Whether the volume `path` is on keeps owners and ACLs (NTFS, ReFS).
/// FAT and exFAT don't. A volume that can't be asked counts as keeping
/// them, so the stricter check applies.
fn keeps_acls(path: &Path) -> bool {
    use windows_sys::Win32::Storage::FileSystem::GetVolumeInformationW;
    // FILE_PERSISTENT_ACLS (SystemServices; named here to keep the feature
    // list short).
    const FILE_PERSISTENT_ACLS: u32 = 8;
    let Some(root) = volume_root(path) else {
        return true;
    };
    let name = wide(&root);
    let mut flags = 0u32;
    // SAFETY: `name` is NUL-terminated; only the flags are asked for, and
    // the other outputs are null with sizes 0.
    let ok = unsafe {
        GetVolumeInformationW(
            name.as_ptr(),
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut flags,
            std::ptr::null_mut(),
            0,
        )
    };
    ok == 0 || flags & FILE_PERSISTENT_ACLS != 0
}

/// Whether the folder `dir` is the user `sid`'s own, as
/// [`create_private_dir`] makes it: owned by that user, with a DACL that
/// is there and protected. Another account can make a folder of that name
/// where `.kiln-held` lets it, but can't make the user its owner. On a
/// volume with no owners or ACLs (FAT, exFAT) there is nothing to check,
/// and it counts as the user's.
pub(super) fn is_users_own(dir: &Path, sid: &str) -> bool {
    use windows_sys::Win32::Security::Authorization::{GetNamedSecurityInfoW, SE_FILE_OBJECT};
    use windows_sys::Win32::Security::{
        GetSecurityDescriptorControl, ACL, DACL_SECURITY_INFORMATION, OWNER_SECURITY_INFORMATION,
        PSID, SE_DACL_PROTECTED,
    };
    if !keeps_acls(dir) {
        return true;
    }
    let name = wide(dir);
    let mut owner: PSID = std::ptr::null_mut();
    let mut dacl: *mut ACL = std::ptr::null_mut();
    let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    // SAFETY: `name` is NUL-terminated. `owner` and `dacl` are set to point
    // inside `sd`, which is freed with `LocalFree` after they are used.
    let err = unsafe {
        GetNamedSecurityInfoW(
            name.as_ptr(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            std::ptr::null_mut(),
            &mut dacl,
            std::ptr::null_mut(),
            &mut sd,
        )
    };
    if err != 0 {
        return false;
    }
    let _free = Local(sd);
    // A null DACL lets everyone in.
    if owner.is_null() || dacl.is_null() {
        return false;
    }
    let (mut control, mut revision) = (0u16, 0u32);
    // SAFETY: `sd` is the valid descriptor just read.
    if unsafe { GetSecurityDescriptorControl(sd, &mut control, &mut revision) } == 0
        || control & SE_DACL_PROTECTED == 0
    {
        return false;
    }
    let mut text: *mut u16 = std::ptr::null_mut();
    // SAFETY: `owner` points inside `sd`, alive here; `text` gets a string
    // to free with `LocalFree`.
    if unsafe { ConvertSidToStringSidW(owner, &mut text) } == 0 {
        return false;
    }
    let _text = Local(text.cast());
    // SAFETY: Windows wrote a NUL-terminated string.
    unsafe { from_wide_ptr(text) }.is_some_and(|o| o.eq_ignore_ascii_case(sid))
}

/// The DACL of `path` in SDDL form, to check what [`create_private_dir`]
/// made and what [`inherit_dacl`] left.
#[cfg(test)]
pub(super) fn dacl_sddl(path: &Path) -> Option<String> {
    use windows_sys::Win32::Security::Authorization::{
        ConvertSecurityDescriptorToStringSecurityDescriptorW, GetNamedSecurityInfoW, SE_FILE_OBJECT,
    };
    use windows_sys::Win32::Security::DACL_SECURITY_INFORMATION;
    let name = wide(path);
    let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    // SAFETY: `name` is NUL-terminated; only the descriptor is asked for,
    // to free with `LocalFree`; the other outputs may be null.
    let err = unsafe {
        GetNamedSecurityInfoW(
            name.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut sd,
        )
    };
    if err != 0 {
        return None;
    }
    let _free = Local(sd);
    let mut text: *mut u16 = std::ptr::null_mut();
    // SAFETY: `sd` is a valid descriptor; `text` gets a string to free
    // with `LocalFree`; the length isn't wanted.
    let ok = unsafe {
        ConvertSecurityDescriptorToStringSecurityDescriptorW(
            sd,
            SDDL_REVISION_1,
            DACL_SECURITY_INFORMATION,
            &mut text,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        return None;
    }
    let _text = Local(text.cast());
    // SAFETY: Windows wrote a NUL-terminated string.
    unsafe { from_wide_ptr(text) }
}

/// Let the file `path` take its permissions from the folder it is in now,
/// as `icacls /reset` does: its DACL is set to an empty one, unprotected,
/// so it holds only the entries its folder hands down. A rename on one
/// volume keeps the DACL a file had where it was made, and a held result
/// was made in a folder only the user can open. On a volume with no ACLs
/// (FAT, exFAT) there is nothing to do.
pub(super) fn inherit_dacl(path: &Path) -> io::Result<()> {
    use windows_sys::Win32::Security::Authorization::{SetNamedSecurityInfoW, SE_FILE_OBJECT};
    use windows_sys::Win32::Security::{
        InitializeAcl, ACL, ACL_REVISION, DACL_SECURITY_INFORMATION,
        UNPROTECTED_DACL_SECURITY_INFORMATION,
    };
    std::fs::symlink_metadata(path)?;
    if !keeps_acls(path) {
        return Ok(());
    }
    // SAFETY: an all-zero ACL is a valid place for `InitializeAcl` to
    // write.
    let mut acl: ACL = unsafe { std::mem::zeroed() };
    // SAFETY: `acl` is writable for its own size, the length passed: an
    // ACL with no entries.
    let size = std::mem::size_of::<ACL>() as u32;
    if unsafe { InitializeAcl(&mut acl, size, ACL_REVISION) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let name = wide(path);
    // SAFETY: `name` is NUL-terminated and `acl` a valid empty ACL, both
    // alive for the call. Only the DACL is set: no owner, group or SACL
    // (null). An empty DACL, not a null one, which would let everyone in.
    let err = unsafe {
        SetNamedSecurityInfoW(
            name.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | UNPROTECTED_DACL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &acl,
            std::ptr::null(),
        )
    };
    if err != 0 {
        return Err(io::Error::from_raw_os_error(err as i32));
    }
    Ok(())
}

/// How a copy ended when it didn't complete.
pub(super) enum CopyEnd {
    Cancelled,
    Failed(io::Error),
}

/// What the progress routine reaches through `lpdata`.
struct Watch<'a> {
    progress: &'a mut dyn FnMut(u64, u64),
    cancel: &'a CancelToken,
    panicked: bool,
}

/// `CopyFileExW`'s progress routine: tell `progress`, then go on unless
/// `cancel` is set. A panic in `progress` stops the copy instead of
/// unwinding through Windows.
unsafe extern "system" fn on_progress(
    total: i64,
    done: i64,
    _stream_size: i64,
    _stream_done: i64,
    _stream: u32,
    _reason: u32,
    _from: HANDLE,
    _to: HANDLE,
    data: *const c_void,
) -> u32 {
    // SAFETY: `data` is the `&mut Watch` `copy_file` passes to
    // `CopyFileExW`, alive for that whole call. Windows calls this routine
    // on the calling thread, during the call, one call at a time, so this
    // is the only reference in use.
    let watch = unsafe { &mut *(data as *mut Watch<'_>) };
    let go = catch_unwind(AssertUnwindSafe(|| {
        (watch.progress)(done.max(0) as u64, total.max(0) as u64);
        !watch.cancel.is_cancelled()
    }));
    match go {
        Ok(true) => PROGRESS_CONTINUE,
        Ok(false) => PROGRESS_CANCEL,
        Err(_) => {
            watch.panicked = true;
            PROGRESS_CANCEL
        }
    }
}

/// Copy `from` to `to` (a temp file Kiln made, which this replaces) with
/// `CopyFileExW`, telling `progress` the bytes copied and the size as it
/// goes, and stopping when `cancel` is set. Windows removes the part
/// copied when the copy stops; the caller removes `to` too, in case.
pub(super) fn copy_file(
    from: &Path,
    to: &Path,
    progress: &mut dyn FnMut(u64, u64),
    cancel: &CancelToken,
) -> Result<(), CopyEnd> {
    if cancel.is_cancelled() {
        return Err(CopyEnd::Cancelled);
    }
    let src = wide(from);
    let dst = wide(to);
    let mut watch = Watch {
        progress,
        cancel,
        panicked: false,
    };
    // SAFETY: both names are NUL-terminated. `watch` outlives the call
    // and is reached only by `on_progress`, on this thread, during it. No
    // cancel flag is passed (null): `on_progress` answers cancel. Flags 0:
    // `to` is Kiln's own temp file, which is replaced.
    let ok = unsafe {
        CopyFileExW(
            src.as_ptr(),
            dst.as_ptr(),
            Some(on_progress),
            &mut watch as *mut Watch<'_> as *const c_void,
            std::ptr::null_mut(),
            0,
        )
    };
    if ok != 0 {
        return Ok(());
    }
    let err = io::Error::last_os_error();
    if watch.panicked {
        return Err(CopyEnd::Failed(io::Error::other(
            "the copy's progress report failed",
        )));
    }
    if err.raw_os_error() == Some(ERROR_REQUEST_ABORTED as i32) && cancel.is_cancelled() {
        return Err(CopyEnd::Cancelled);
    }
    Err(CopyEnd::Failed(err))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_paths_get_the_prefix_and_lose_it_again() {
        let long = format!(r"C:\{}", "a\\".repeat(130));
        let w = wide(Path::new(&long));
        let text = String::from_utf16(&w[..w.len() - 1]).unwrap();
        assert!(text.starts_with(r"\\?\C:\"), "{text}");
        assert_eq!(plain(text), long);

        let unc = format!(r"\\server\share\{}", "b\\".repeat(130));
        let w = wide(Path::new(&unc));
        let text = String::from_utf16(&w[..w.len() - 1]).unwrap();
        assert!(text.starts_with(r"\\?\UNC\server\share\"), "{text}");
        assert_eq!(plain(text), unc);

        let short = wide(Path::new(r"C:\x"));
        assert_eq!(
            String::from_utf16(&short[..short.len() - 1]).unwrap(),
            r"C:\x"
        );
    }

    #[test]
    fn a_private_folder_is_only_the_users() {
        let sid = user_sid().expect("this process's SID");
        assert!(is_sid(&sid), "{sid}");
        let scratch = tempfile::tempdir().unwrap();
        let dir = scratch.path().join(&sid);
        create_private_dir(&dir, &sid).unwrap();
        let sddl = dacl_sddl(&dir).unwrap();
        // Protected, so nothing comes from the folder above, and three
        // entries: the user, SYSTEM and Administrators.
        assert!(sddl.starts_with("D:P"), "{sddl}");
        assert_eq!(sddl.matches("(A;").count(), 3, "{sddl}");
        assert!(sddl.contains(&format!(";;;{sid})")), "{sddl}");
        assert!(sddl.contains(";;;SY)") && sddl.contains(";;;BA)"), "{sddl}");
        assert!(is_users_own(&dir, &sid));
        // A folder made the plain way inherits its DACL: not the user's own
        // in this sense, though the user made it.
        let plain = scratch.path().join("plain");
        std::fs::create_dir(&plain).unwrap();
        assert!(!is_users_own(&plain, &sid));
        // Owned by the user, but by another SID's name: not that SID's.
        assert!(!is_users_own(&dir, "S-1-5-21-9-9-9-4242"));
        // What Kiln makes inside is the user's still.
        std::fs::write(dir.join("a.txt"), "a").unwrap();
        assert!(create_private_dir(&dir, &sid).is_err());
        assert!(create_private_dir(&scratch.path().join("x"), "not a sid").is_err());
    }

    #[test]
    fn a_rename_never_replaces() {
        let scratch = tempfile::tempdir().unwrap();
        let (a, b) = (scratch.path().join("a"), scratch.path().join("b"));
        std::fs::write(&a, "a").unwrap();
        std::fs::write(&b, "b").unwrap();
        let err = rename_new(&a, &b).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "a");
        assert_eq!(std::fs::read_to_string(&b).unwrap(), "b");
        rename_new(&a, &scratch.path().join("c")).unwrap();
        assert!(!a.exists());
    }

    #[test]
    fn the_temp_folder_is_on_a_volume() {
        let dir = tempfile::tempdir().unwrap();
        let root = volume_root(&dir.path().join("not yet").join("x.jpg")).unwrap();
        assert!(dir.path().starts_with(&root), "{}", root.display());
        assert!(root.to_string_lossy().ends_with('\\'));
    }
}
