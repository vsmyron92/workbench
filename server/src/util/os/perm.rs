//! `perm`: private files and permission bits.
//!
//! Unix keeps its modes. On Windows a private mode (no group or other bits, e.g. 0600, 0700)
//! becomes a protected DACL granting full control to the current user and SYSTEM only (on a
//! directory also inherited, so everything created inside is born private), set when the file
//! or directory is created; any other mode lets it inherit its folder's ACL. Windows has no
//! mode bits to keep or copy: a file replacing another gets that file's DACL instead.

use std::fs::{File, Metadata};
use std::io;
use std::path::Path;

/// Whether other users can read a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Privacy {
    /// Only its owner (on Windows also SYSTEM and Administrators).
    Private,
    /// Others can read it; why, for messages (`mode 644`, `BUILTIN\Users can read it`).
    Exposed(String),
}

impl Privacy {
    /// Whether others can read it.
    pub fn is_exposed(&self) -> bool {
        matches!(self, Privacy::Exposed(_))
    }
}

/// How to make a file private, for messages.
#[cfg(unix)]
pub const MAKE_PRIVATE: &str = "run chmod 600";
#[cfg(windows)]
pub const MAKE_PRIVATE: &str = "use Settings → Secrets to make it private";

/// What making a file private (`apply(path, 0o600)`) is called in an error message:
/// `<this> <file>: <error>`.
#[cfg(unix)]
pub const MAKING_PRIVATE: &str = "chmod";
#[cfg(windows)]
pub const MAKING_PRIVATE: &str = "make private";

/// Set `path`'s permission bits (chmod). Windows: a private mode gives it the private DACL;
/// any other mode leaves its ACL as it is.
pub fn apply(path: &Path, mode: u32) -> io::Result<()> {
    imp::apply(path, mode)
}

/// `apply` on an open file (fchmod).
pub fn apply_to(file: &File, mode: u32) -> io::Result<()> {
    imp::apply_to(file, mode)
}

/// The permission bits of `meta` (`st_mode`); `None` on Windows.
pub fn mode(meta: &Metadata) -> Option<u32> {
    imp::mode(meta)
}

/// Create the directory `path` and any missing parents, private (0700). Existing ones are
/// left as they are.
pub fn create_dir_private(path: &Path) -> io::Result<()> {
    imp::create_dir_private(path)
}

/// Create the file `path` for writing, failing if anything exists there, even a dangling
/// symlink; `mode` applies from the start. `nofollow` adds `O_NOFOLLOW` on Unix, as the
/// callers always did; creating never follows a symlink at `path` anyway.
pub fn open_new(path: &Path, mode: u32, nofollow: bool) -> io::Result<File> {
    imp::open_new(path, mode, nofollow)
}

/// Open `path` for appending, creating it with `mode` if it does not exist.
pub fn open_append(path: &Path, mode: u32) -> io::Result<File> {
    imp::open_append(path, mode)
}

/// `file.set_len(len)`, also for a file from `open_append` (whose Windows handle may only
/// append).
pub fn set_len(file: &File, len: u64) -> io::Result<()> {
    imp::set_len(file, len)
}

/// Create `tmp` (which must not exist) to be renamed over `target` later
/// (`rename_into_place`). It gets `target`'s permissions when `target` exists (Windows: its
/// DACL), else `mode` (`None`: the default for new files), before anything is written.
pub fn create_replacement(tmp: &Path, target: &Path, mode: Option<u32>) -> io::Result<File> {
    imp::create_replacement(tmp, target, mode)
}

/// Rename `tmp` over `target`. Windows retries for half a second while another process (an
/// indexer, antivirus, an editor) briefly holds `target`.
pub fn rename_into_place(tmp: &Path, target: &Path) -> io::Result<()> {
    imp::rename_into_place(tmp, target)
}

/// Whether users other than the owner can read `path` (Windows: anyone but the owner, the
/// current user, SYSTEM and Administrators).
pub fn privacy(path: &Path) -> io::Result<Privacy> {
    imp::privacy(path)
}

/// `path`'s permissions as shown to the user: the octal mode (`600`); on Windows `private` or
/// `shared`.
pub fn describe(path: &Path) -> io::Result<String> {
    imp::describe(path)
}

/// Whether the current user owns `path` (Windows: also Administrators, when this process is
/// an elevated administrator).
pub fn owned_by_me(path: &Path) -> io::Result<bool> {
    imp::owned_by_me(path)
}

/// The current user's uid and gid (a dev container's user is mapped onto them); `None` on
/// Windows, which has neither.
pub fn user_ids() -> Option<(u32, u32)> {
    imp::user_ids()
}

/// Tests: `path` has the permission bits `mode` on Unix; a private mode is also checked
/// through `privacy`, which is what Windows can check.
#[cfg(test)]
#[track_caller]
pub fn assert_mode(path: &Path, mode: u32) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let actual = std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(actual, mode, "mode of {}: {actual:o}, not {mode:o}", path.display());
    }
    if mode & 0o077 == 0 {
        let p = privacy(path).unwrap();
        assert_eq!(p, Privacy::Private, "{} is not private", path.display());
    }
}

/// Tests: make `path` readable by other users (Unix: chmod `mode`; Windows: an entry for
/// Everyone).
#[cfg(test)]
pub fn expose(path: &Path, mode: u32) {
    imp::expose(path, mode)
}

#[cfg(unix)]
mod imp {
    use std::fs::{File, Metadata, OpenOptions, Permissions};
    use std::io;
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
    use std::path::Path;

    use super::Privacy;

    pub fn apply(path: &Path, mode: u32) -> io::Result<()> {
        std::fs::set_permissions(path, Permissions::from_mode(mode))
    }

    pub fn apply_to(file: &File, mode: u32) -> io::Result<()> {
        file.set_permissions(Permissions::from_mode(mode))
    }

    pub fn mode(meta: &Metadata) -> Option<u32> {
        Some(meta.permissions().mode())
    }

    pub fn create_dir_private(path: &Path) -> io::Result<()> {
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(path)
    }

    pub fn open_new(path: &Path, mode: u32, nofollow: bool) -> io::Result<File> {
        let mut o = OpenOptions::new();
        o.write(true).create_new(true).mode(mode);
        if nofollow {
            o.custom_flags(libc::O_NOFOLLOW);
        }
        o.open(path)
    }

    pub fn open_append(path: &Path, mode: u32) -> io::Result<File> {
        OpenOptions::new().create(true).append(true).mode(mode).open(path)
    }

    pub fn set_len(file: &File, len: u64) -> io::Result<()> {
        file.set_len(len)
    }

    pub fn create_replacement(tmp: &Path, target: &Path, mode: Option<u32>) -> io::Result<File> {
        let keep_mode = std::fs::metadata(target).ok().map(|m| m.permissions().mode());
        let f = OpenOptions::new().write(true).create_new(true).open(tmp)?;
        if let Some(m) = keep_mode.or(mode) {
            f.set_permissions(Permissions::from_mode(m & 0o7777))?;
        }
        Ok(f)
    }

    pub fn rename_into_place(tmp: &Path, target: &Path) -> io::Result<()> {
        std::fs::rename(tmp, target)
    }

    pub fn privacy(path: &Path) -> io::Result<Privacy> {
        let mode = std::fs::metadata(path)?.permissions().mode();
        Ok(if mode & 0o077 != 0 { Privacy::Exposed(format!("mode {:o}", mode & 0o777)) } else { Privacy::Private })
    }

    pub fn describe(path: &Path) -> io::Result<String> {
        Ok(format!("{:o}", std::fs::metadata(path)?.permissions().mode() & 0o777))
    }

    pub fn owned_by_me(path: &Path) -> io::Result<bool> {
        Ok(std::fs::metadata(path)?.uid() == nix::unistd::getuid().as_raw())
    }

    pub fn user_ids() -> Option<(u32, u32)> {
        Some((nix::unistd::getuid().as_raw(), nix::unistd::getgid().as_raw()))
    }

    #[cfg(test)]
    pub fn expose(path: &Path, mode: u32) {
        apply(path, mode).unwrap();
    }
}

#[cfg(windows)]
mod imp {
    use std::ffi::c_void;
    use std::fs::{File, Metadata};
    use std::io;
    use std::mem::{offset_of, size_of};
    use std::os::windows::io::{AsRawHandle, FromRawHandle};
    use std::path::Path;
    use std::ptr::{null, null_mut};
    use std::sync::OnceLock;
    use std::time::Duration;

    use windows_sys::Win32::Foundation::{
        ERROR_ACCESS_DENIED, ERROR_LOCK_VIOLATION, ERROR_SHARING_VIOLATION, ERROR_SUCCESS, GENERIC_ALL, GENERIC_READ, GENERIC_WRITE,
        INVALID_HANDLE_VALUE, WIN32_ERROR,
    };
    use windows_sys::Win32::Security::Authorization::{GetNamedSecurityInfoW, SE_FILE_OBJECT, SetNamedSecurityInfoW, SetSecurityInfo};
    use windows_sys::Win32::Security::{
        ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, ACL_REVISION, AddAccessAllowedAceEx, AddAce, CONTAINER_INHERIT_ACE, CheckTokenMembership, CopySid,
        CreateWellKnownSid, DACL_SECURITY_INFORMATION, EqualSid, GetAce, GetLengthSid, GetSecurityDescriptorControl, INHERIT_ONLY_ACE, INHERITED_ACE,
        InitializeAcl, InitializeSecurityDescriptor, LookupAccountSidW, OBJECT_INHERIT_ACE, OBJECT_SECURITY_INFORMATION, OWNER_SECURITY_INFORMATION,
        PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, SE_DACL_PROTECTED, SECURITY_ATTRIBUTES, SECURITY_DESCRIPTOR,
        SECURITY_MAX_SID_SIZE, SetSecurityDescriptorControl, SetSecurityDescriptorDacl, UNPROTECTED_DACL_SECURITY_INFORMATION, WELL_KNOWN_SID_TYPE,
        WinBuiltinAdministratorsSid, WinCreatorOwnerRightsSid, WinLocalSystemSid,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CREATE_NEW, CreateDirectoryW, CreateFileW, FILE_ALL_ACCESS, FILE_ATTRIBUTE_NORMAL, FILE_CREATION_DISPOSITION,
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_GENERIC_WRITE, FILE_READ_ATTRIBUTES, FILE_READ_DATA, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        FILE_WRITE_DATA, OPEN_ALWAYS, READ_CONTROL, ReOpenFile, WRITE_DAC, WRITE_OWNER,
    };
    use windows_sys::Win32::System::SystemServices::{
        ACCESS_ALLOWED_ACE_TYPE, ACCESS_ALLOWED_CALLBACK_ACE_TYPE, SECURITY_DESCRIPTOR_REVISION,
    };

    use super::Privacy;
    use crate::util::os::win32::{Local, User, sid_string, wide_path};

    /// What a new file's handle may do: write (as std's) and read its attributes (`metadata`).
    /// Not change its DACL: a share may refuse that right (`apply_to` asks for it itself).
    const NEW_FILE_ACCESS: u32 = GENERIC_WRITE | FILE_READ_ATTRIBUTES;

    /// Rights that let an account read a file (or get itself the right to).
    const READ_RIGHTS: u32 = FILE_READ_DATA | GENERIC_READ | GENERIC_ALL | WRITE_DAC | WRITE_OWNER;

    fn private(mode: u32) -> bool {
        mode & 0o077 == 0
    }

    pub fn apply(path: &Path, mode: u32) -> io::Result<()> {
        if !private(mode) {
            return Ok(());
        }
        let dir = std::fs::metadata(path)?.is_dir();
        // Setting a directory's DACL also rewrites what its whole tree inherits: skip it when
        // it is private already (the data dir, at every start).
        if already_private(path, dir).unwrap_or(false) {
            return Ok(());
        }
        set_dacl(path, &private_acl(dir)?, true)
    }

    pub fn apply_to(file: &File, mode: u32) -> io::Result<()> {
        if !private(mode) {
            return Ok(());
        }
        let acl = private_acl(false)?;
        let h = reopen(file, READ_CONTROL | WRITE_DAC)?;
        let info = DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION;
        // SAFETY: a valid handle for the call's duration; the ACL is ours and well formed.
        let rc = unsafe { SetSecurityInfo(h.as_raw_handle(), SE_FILE_OBJECT, info, null_mut(), null_mut(), acl.ptr(), null()) };
        check(rc)
    }

    pub fn mode(_meta: &Metadata) -> Option<u32> {
        None
    }

    pub fn create_dir_private(path: &Path) -> io::Result<()> {
        let sd = Descriptor::new(Some(private_acl(true)?), true)?;
        create_dirs(path, &sd)
    }

    /// std's `create_dir_all`, creating each missing directory with `sd`.
    fn create_dirs(path: &Path, sd: &Descriptor) -> io::Result<()> {
        if path == Path::new("") {
            return Ok(());
        }
        match create_dir(path, sd) {
            Ok(()) => return Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(_) if path.is_dir() => return Ok(()),
            Err(e) => return Err(e),
        }
        match path.parent() {
            Some(parent) => create_dirs(parent, sd)?,
            None => return Err(io::Error::other("failed to create whole tree")),
        }
        match create_dir(path, sd) {
            Ok(()) => Ok(()),
            Err(_) if path.is_dir() => Ok(()),
            Err(e) => Err(e),
        }
    }

    fn create_dir(path: &Path, sd: &Descriptor) -> io::Result<()> {
        let w = wide_path(path)?;
        let sa = sd.attributes();
        // SAFETY: `w` is NUL-terminated; `sa` and the descriptor it points to outlive the call.
        if unsafe { CreateDirectoryW(w.as_ptr(), &sa) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// `nofollow` changes nothing: `CREATE_NEW` never goes through a symlink (`create_file`).
    pub fn open_new(path: &Path, mode: u32, _nofollow: bool) -> io::Result<File> {
        let sd = for_mode(mode)?;
        create_file(path, sd.as_ref(), NEW_FILE_ACCESS, CREATE_NEW)
    }

    pub fn open_append(path: &Path, mode: u32) -> io::Result<File> {
        let sd = for_mode(mode)?;
        // As std opens for appending: every write goes to the end.
        create_file(path, sd.as_ref(), FILE_GENERIC_WRITE & !FILE_WRITE_DATA, OPEN_ALWAYS)
    }

    pub fn set_len(file: &File, len: u64) -> io::Result<()> {
        reopen(file, GENERIC_WRITE)?.set_len(len)
    }

    pub fn create_replacement(tmp: &Path, target: &Path, mode: Option<u32>) -> io::Result<File> {
        let sd = match like(target) {
            Ok(sd) => sd,
            Err(_) => match mode {
                Some(m) => for_mode(m)?,
                None => None,
            },
        };
        create_file(tmp, sd.as_ref(), NEW_FILE_ACCESS, CREATE_NEW)
    }

    pub fn rename_into_place(tmp: &Path, target: &Path) -> io::Result<()> {
        let mut attempt = 0;
        loop {
            match std::fs::rename(tmp, target) {
                // Sharing violations; one being deleted or scanned reports access denied.
                Err(e) if attempt < 10 && e.raw_os_error().is_some_and(|c| matches!(c as u32, ERROR_SHARING_VIOLATION | ERROR_LOCK_VIOLATION | ERROR_ACCESS_DENIED)) => {
                    attempt += 1;
                    std::thread::sleep(Duration::from_millis(50));
                }
                r => return r,
            }
        }
    }

    pub fn privacy(path: &Path) -> io::Result<Privacy> {
        let s = Security::of(path, OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION)?;
        if s.dacl.is_null() {
            return Ok(Privacy::Exposed("it has no DACL, so everyone has full access".into()));
        }
        let mut names: Vec<String> = vec![];
        for sid in readers(&s, false)? {
            let name = account_name(sid);
            if !names.contains(&name) {
                names.push(name);
            }
        }
        Ok(if names.is_empty() { Privacy::Private } else { Privacy::Exposed(format!("{} can read it", names.join(", "))) })
    }

    pub fn describe(path: &Path) -> io::Result<String> {
        // As `privacy`, without looking up names.
        let s = Security::of(path, OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION)?;
        let shared = s.dacl.is_null() || !readers(&s, false)?.is_empty();
        Ok(if shared { "shared" } else { "private" }.into())
    }

    pub fn owned_by_me(path: &Path) -> io::Result<bool> {
        let s = Security::of(path, OWNER_SECURITY_INFORMATION)?;
        let sids = sids()?;
        if s.owner.is_null() {
            return Ok(false);
        }
        // SAFETY: valid SIDs (the owner points into `s`).
        if unsafe { EqualSid(s.owner, sids.user.ptr()) } != 0 {
            return Ok(true);
        }
        // What an elevated administrator creates belongs to Administrators.
        let mut member = 0;
        // SAFETY: valid SIDs; a null token checks this thread's (or this process's) token.
        let admin = unsafe { EqualSid(s.owner, sids.admins.ptr()) != 0 && CheckTokenMembership(null_mut(), sids.admins.ptr(), &mut member) != 0 };
        Ok(admin && member != 0)
    }

    pub fn user_ids() -> Option<(u32, u32)> {
        None
    }

    #[cfg(test)]
    pub fn expose(path: &Path, _mode: u32) {
        use windows_sys::Win32::Security::WinWorldSid;
        use windows_sys::Win32::Storage::FileSystem::FILE_GENERIC_READ;
        let (user, everyone) = (&sids().unwrap().user, Sid::well_known(WinWorldSid).unwrap());
        let mut acl = Acl::new(size_of::<ACL>() + ace_size(user) + ace_size(&everyone), ACL_REVISION).unwrap();
        acl.allow(user, FILE_ALL_ACCESS, 0).unwrap();
        acl.allow(&everyone, FILE_GENERIC_READ, 0).unwrap();
        set_dacl(path, &acl, false).unwrap();
    }

    // ---------------------------------------------------------------- SIDs

    /// A SID in a buffer of our own (u32s: a SID's sub-authorities are 32-bit).
    struct Sid(Vec<u32>);

    impl Sid {
        fn ptr(&self) -> PSID {
            self.0.as_ptr() as PSID
        }

        fn len(&self) -> u32 {
            // SAFETY: a valid SID we own.
            unsafe { GetLengthSid(self.ptr()) }
        }

        fn copy(sid: PSID) -> io::Result<Sid> {
            // SAFETY: the caller passes a valid SID.
            let len = unsafe { GetLengthSid(sid) };
            let mut buf = vec![0u32; (len as usize).div_ceil(4)];
            // SAFETY: `buf` holds `len` bytes.
            if unsafe { CopySid(len, buf.as_mut_ptr().cast(), sid) } == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(Sid(buf))
        }

        fn well_known(kind: WELL_KNOWN_SID_TYPE) -> io::Result<Sid> {
            let mut len = SECURITY_MAX_SID_SIZE;
            let mut buf = vec![0u32; len as usize / 4];
            // SAFETY: `buf` holds `len` bytes; these types need no domain SID.
            if unsafe { CreateWellKnownSid(kind, null_mut(), buf.as_mut_ptr().cast(), &mut len) } == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(Sid(buf))
        }
    }

    struct Sids {
        user: Sid,
        system: Sid,
        admins: Sid,
        owner_rights: Sid,
    }

    /// The current user's SID and the well-known ones, looked up once.
    fn sids() -> io::Result<&'static Sids> {
        static SIDS: OnceLock<Sids> = OnceLock::new();
        if let Some(s) = SIDS.get() {
            return Ok(s);
        }
        let s = Sids {
            user: current_user()?,
            system: Sid::well_known(WinLocalSystemSid)?,
            admins: Sid::well_known(WinBuiltinAdministratorsSid)?,
            owner_rights: Sid::well_known(WinCreatorOwnerRightsSid)?,
        };
        Ok(SIDS.get_or_init(|| s))
    }

    fn current_user() -> io::Result<Sid> {
        Sid::copy(User::current()?.sid())
    }

    /// `DOMAIN\name`, or the SID's string form.
    fn account_name(sid: PSID) -> String {
        let (mut name, mut domain) = ([0u16; 256], [0u16; 256]);
        let (mut n, mut d, mut kind) = (256u32, 256u32, 0);
        // SAFETY: the buffers hold the sizes given; `sid` is valid.
        if unsafe { LookupAccountSidW(null(), sid, name.as_mut_ptr(), &mut n, domain.as_mut_ptr(), &mut d, &mut kind) } != 0 {
            let name = String::from_utf16_lossy(&name[..n as usize]);
            let domain = String::from_utf16_lossy(&domain[..d as usize]);
            return if domain.is_empty() { name } else { format!("{domain}\\{name}") };
        }
        // SAFETY: `sid` is valid.
        unsafe { sid_string(sid) }.unwrap_or_else(|| "an unknown account".into())
    }

    // ---------------------------------------------------------------- DACLs

    /// An ACL in a buffer of our own (u32s: ACLs are DWORD-aligned).
    struct Acl(Vec<u32>);

    impl Acl {
        fn new(size: usize, revision: u32) -> io::Result<Acl> {
            let size = size.next_multiple_of(4);
            let mut buf = vec![0u32; size / 4];
            // SAFETY: `buf` holds `size` bytes.
            if unsafe { InitializeAcl(buf.as_mut_ptr().cast(), size as u32, revision) } == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(Acl(buf))
        }

        fn ptr(&self) -> *const ACL {
            self.0.as_ptr().cast()
        }

        fn allow(&mut self, sid: &Sid, mask: u32, flags: u32) -> io::Result<()> {
            // SAFETY: an initialized ACL sized for this entry; `sid` is valid.
            if unsafe { AddAccessAllowedAceEx(self.0.as_mut_ptr().cast(), ACL_REVISION, flags, mask, sid.ptr()) } == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        }

        fn add(&mut self, ace: &Ace, revision: u32) -> io::Result<()> {
            // SAFETY: `ace.raw` is a whole ACE of `ace.size` bytes; the ACL was sized for it.
            if unsafe { AddAce(self.0.as_mut_ptr().cast(), revision, u32::MAX, ace.raw, ace.size.into()) } == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        }
    }

    fn ace_size(sid: &Sid) -> usize {
        offset_of!(ACCESS_ALLOWED_ACE, SidStart) + sid.len() as usize
    }

    /// Full control for the current user and SYSTEM, and nobody else; inherited by what is
    /// created inside a directory.
    fn private_acl(dir: bool) -> io::Result<Acl> {
        let s = sids()?;
        let mut acl = Acl::new(size_of::<ACL>() + ace_size(&s.user) + ace_size(&s.system), ACL_REVISION)?;
        let flags = if dir { OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE } else { 0 };
        acl.allow(&s.user, FILE_ALL_ACCESS, flags)?;
        acl.allow(&s.system, FILE_ALL_ACCESS, flags)?;
        Ok(acl)
    }

    /// Set `path`'s DACL; `protected` stops it inheriting from the folder.
    fn set_dacl(path: &Path, acl: &Acl, protected: bool) -> io::Result<()> {
        let w = wide_path(path)?;
        let info = DACL_SECURITY_INFORMATION | if protected { PROTECTED_DACL_SECURITY_INFORMATION } else { UNPROTECTED_DACL_SECURITY_INFORMATION };
        // SAFETY: `w` is NUL-terminated; the ACL is ours and well formed.
        check(unsafe { SetNamedSecurityInfoW(w.as_ptr(), SE_FILE_OBJECT, info, null_mut(), null_mut(), acl.ptr(), null()) })
    }

    /// One entry of a DACL.
    pub(super) struct Ace {
        kind: u32,
        pub(super) flags: u32,
        size: u16,
        /// The access mask and SID of a (callback) allow entry; 0 and null for other entries.
        mask: u32,
        sid: PSID,
        raw: *const c_void,
    }

    /// The entries of `acl` (none for a NULL DACL).
    pub(super) fn aces(acl: *const ACL) -> Vec<Ace> {
        let mut out = vec![];
        if acl.is_null() {
            return out;
        }
        // SAFETY: a valid ACL inside a security descriptor the caller holds.
        let count = unsafe { std::ptr::read_unaligned(acl) }.AceCount;
        for i in 0..u32::from(count) {
            let mut raw: *mut c_void = null_mut();
            // SAFETY: `i` is below the entry count.
            if unsafe { GetAce(acl, i, &mut raw) } == 0 {
                continue;
            }
            // SAFETY: every entry starts with a header.
            let h = unsafe { std::ptr::read_unaligned(raw as *const ACE_HEADER) };
            let kind = u32::from(h.AceType);
            // An allow entry with room for its mask and a SID (8 bytes at least).
            let sid_at = offset_of!(ACCESS_ALLOWED_ACE, SidStart);
            let allow = matches!(kind, ACCESS_ALLOWED_ACE_TYPE | ACCESS_ALLOWED_CALLBACK_ACE_TYPE) && usize::from(h.AceSize) >= sid_at + 8;
            let (mask, sid) = if allow {
                // SAFETY: the entry holds a whole ACCESS_ALLOWED_ACE (checked above).
                let ace = unsafe { std::ptr::read_unaligned(raw as *const ACCESS_ALLOWED_ACE) };
                // SAFETY: within the entry (see above).
                (ace.Mask, unsafe { raw.cast::<u8>().add(sid_at) }.cast())
            } else {
                (0, null_mut())
            };
            out.push(Ace { kind, flags: h.AceFlags.into(), size: h.AceSize, mask, sid, raw });
        }
        out
    }

    /// The SIDs (pointing into `s`) other than the owner, the current user, SYSTEM and
    /// Administrators that an entry lets read the file; with `children`, also those an entry
    /// only passes on to what is created inside a directory.
    fn readers(s: &Security, children: bool) -> io::Result<Vec<PSID>> {
        let sids = sids()?;
        let trusted = [s.owner, sids.user.ptr(), sids.system.ptr(), sids.admins.ptr(), sids.owner_rights.ptr()];
        let inherits = OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE;
        Ok(aces(s.dacl)
            .into_iter()
            .filter(|a| matches!(a.kind, ACCESS_ALLOWED_ACE_TYPE | ACCESS_ALLOWED_CALLBACK_ACE_TYPE))
            .filter(|a| a.flags & INHERIT_ONLY_ACE == 0 || children && a.flags & inherits != 0)
            .filter(|a| a.mask & READ_RIGHTS != 0)
            // SAFETY: valid SIDs.
            .filter(|a| !trusted.iter().any(|t| !t.is_null() && unsafe { EqualSid(*t, a.sid) } != 0))
            .map(|a| a.sid)
            .collect())
    }

    /// A protected DACL nobody else can read through (on a directory, also passed on to what
    /// is created inside).
    fn already_private(path: &Path, dir: bool) -> io::Result<bool> {
        let s = Security::of(path, OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION)?;
        if s.dacl.is_null() || !s.protected() {
            return Ok(false);
        }
        let both = OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE;
        if dir && !aces(s.dacl).iter().any(|a| a.flags & both == both) {
            return Ok(false);
        }
        Ok(readers(&s, dir)?.is_empty())
    }

    // ---------------------------------------------------------------- descriptors

    /// A security descriptor to create a file or directory with.
    struct Descriptor {
        sd: Box<SECURITY_DESCRIPTOR>,
        _acl: Option<Acl>,
    }

    impl Descriptor {
        /// `acl` `None`: a NULL DACL (everyone has full access).
        fn new(acl: Option<Acl>, protected: bool) -> io::Result<Descriptor> {
            let mut sd = Box::<SECURITY_DESCRIPTOR>::default();
            let p: PSECURITY_DESCRIPTOR = (&mut *sd as *mut SECURITY_DESCRIPTOR).cast();
            let dacl = acl.as_ref().map_or(null(), Acl::ptr);
            // SAFETY: `p` is a descriptor we own; the ACL's buffer lives (and stays put) in
            // `Self` as long as the descriptor does.
            let ok = unsafe {
                InitializeSecurityDescriptor(p, SECURITY_DESCRIPTOR_REVISION) != 0
                    && SetSecurityDescriptorDacl(p, 1, dacl, 0) != 0
                    && (!protected || SetSecurityDescriptorControl(p, SE_DACL_PROTECTED, SE_DACL_PROTECTED) != 0)
            };
            if !ok {
                return Err(io::Error::last_os_error());
            }
            Ok(Descriptor { sd, _acl: acl })
        }

        fn attributes(&self) -> SECURITY_ATTRIBUTES {
            SECURITY_ATTRIBUTES {
                nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: (&*self.sd as *const SECURITY_DESCRIPTOR).cast_mut().cast(),
                bInheritHandle: 0,
            }
        }
    }

    /// The private DACL for a private mode, else `None` (inherit from the folder).
    fn for_mode(mode: u32) -> io::Result<Option<Descriptor>> {
        if !private(mode) {
            return Ok(None);
        }
        Ok(Some(Descriptor::new(Some(private_acl(false)?), true)?))
    }

    /// A descriptor giving a new file beside `target` the DACL `target` has: its own entries
    /// and whether it is protected; inherited entries come again from the folder. `None`: it
    /// has no entries of its own, so the new file only inherits, as `target` did.
    fn like(target: &Path) -> io::Result<Option<Descriptor>> {
        let s = Security::of(target, DACL_SECURITY_INFORMATION)?;
        if s.dacl.is_null() {
            return Ok(Some(Descriptor::new(None, false)?));
        }
        let protected = s.protected();
        let own: Vec<Ace> = aces(s.dacl).into_iter().filter(|a| protected || a.flags & INHERITED_ACE == 0).collect();
        if own.is_empty() && !protected {
            return Ok(None);
        }
        // SAFETY: a valid ACL inside `s`.
        let revision = u32::from(unsafe { std::ptr::read_unaligned(s.dacl) }.AclRevision);
        let mut acl = Acl::new(size_of::<ACL>() + own.iter().map(|a| usize::from(a.size)).sum::<usize>(), revision)?;
        for a in &own {
            acl.add(a, revision)?;
        }
        Ok(Some(Descriptor::new(Some(acl), protected)?))
    }

    /// A file's owner and DACL, as far as asked for (the pointers point into the block).
    pub(super) struct Security {
        block: Local,
        owner: PSID,
        pub(super) dacl: *const ACL,
    }

    impl Security {
        pub(super) fn of(path: &Path, what: OBJECT_SECURITY_INFORMATION) -> io::Result<Security> {
            let w = wide_path(path)?;
            let (mut owner, mut dacl, mut sd): (PSID, *mut ACL, PSECURITY_DESCRIPTOR) = (null_mut(), null_mut(), null_mut());
            // SAFETY: `w` is NUL-terminated; the results point into `sd`, which `Local` frees.
            let rc = unsafe { GetNamedSecurityInfoW(w.as_ptr(), SE_FILE_OBJECT, what, &mut owner, null_mut(), &mut dacl, null_mut(), &mut sd) };
            check(rc)?;
            Ok(Security { block: Local(sd), owner, dacl })
        }

        pub(super) fn protected(&self) -> bool {
            let (mut control, mut revision) = (0u16, 0u32);
            // SAFETY: a valid descriptor from GetNamedSecurityInfoW.
            let ok = unsafe { GetSecurityDescriptorControl(self.block.0, &mut control, &mut revision) } != 0;
            ok && control & SE_DACL_PROTECTED != 0
        }
    }

    // ---------------------------------------------------------------- handles and paths

    /// Another handle to `file`, with the rights `access`.
    fn reopen(file: &File, access: u32) -> io::Result<File> {
        let share = FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE;
        // SAFETY: a valid handle for the call's duration.
        let h = unsafe { ReOpenFile(file.as_raw_handle(), access, share, 0) };
        if h == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: a fresh handle nobody else owns; `File` closes it.
        Ok(unsafe { File::from_raw_handle(h) })
    }

    fn check(rc: WIN32_ERROR) -> io::Result<()> {
        if rc == ERROR_SUCCESS { Ok(()) } else { Err(io::Error::from_raw_os_error(rc as i32)) }
    }

    fn create_file(path: &Path, sd: Option<&Descriptor>, access: u32, disposition: FILE_CREATION_DISPOSITION) -> io::Result<File> {
        let w = wide_path(path)?;
        let sa = sd.map(Descriptor::attributes);
        let sa_ptr = sa.as_ref().map_or(null(), |a| a as *const SECURITY_ATTRIBUTES);
        // As std's `create_new`, a new file is never created through a symlink or junction,
        // even a dangling one (Unix: `O_EXCL`). Appending follows them, as `O_APPEND` does.
        let flags = FILE_ATTRIBUTE_NORMAL | if disposition == CREATE_NEW { FILE_FLAG_OPEN_REPARSE_POINT } else { 0 };
        // std's default sharing, so the file behaves like one std opened.
        let share = FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE;
        // SAFETY: `w` is NUL-terminated; `sa` and the descriptor it points to outlive the call.
        let h = unsafe { CreateFileW(w.as_ptr(), access, share, sa_ptr, disposition, flags, null_mut()) };
        if h == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: a fresh handle nobody else owns; `File` closes it.
        Ok(unsafe { File::from_raw_handle(h) })
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// A directory whose entry passes read access on to new files only is not private:
        /// `apply` protects it again, so what is created inside is born private.
        #[test]
        fn inherit_only_readers_make_a_directory_shared() {
            use windows_sys::Win32::Security::WinWorldSid;
            use windows_sys::Win32::Storage::FileSystem::FILE_GENERIC_READ;
            let d = tempfile::tempdir().unwrap();
            let dir = d.path().join("d");
            create_dir_private(&dir).unwrap();
            assert!(already_private(&dir, true).unwrap());

            let (user, everyone) = (&sids().unwrap().user, Sid::well_known(WinWorldSid).unwrap());
            let mut acl = Acl::new(size_of::<ACL>() + ace_size(user) + ace_size(&everyone), ACL_REVISION).unwrap();
            acl.allow(user, FILE_ALL_ACCESS, OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE).unwrap();
            acl.allow(&everyone, FILE_GENERIC_READ, OBJECT_INHERIT_ACE | INHERIT_ONLY_ACE).unwrap();
            set_dacl(&dir, &acl, true).unwrap();
            assert_eq!(privacy(&dir).unwrap(), Privacy::Private, "the directory itself");
            assert!(!already_private(&dir, true).unwrap());
            std::fs::write(dir.join("before"), "x").unwrap();
            assert!(privacy(&dir.join("before")).unwrap().is_exposed());

            apply(&dir, 0o700).unwrap();
            std::fs::write(dir.join("after"), "x").unwrap();
            assert_eq!(privacy(&dir.join("after")).unwrap(), Privacy::Private);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_files_are_exclusive_and_get_their_mode_from_the_start() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("secret");
        let mut f = open_new(&p, 0o600, false).unwrap();
        assert_mode(&p, 0o600);
        std::io::Write::write_all(&mut f, b"x").unwrap();
        drop(f);
        assert_eq!(open_new(&p, 0o600, true).unwrap_err().kind(), io::ErrorKind::AlreadyExists);
        assert!(owned_by_me(&p).unwrap());

        let log = d.path().join("log");
        for line in ["a\n", "b\n"] {
            std::io::Write::write_all(&mut open_append(&log, 0o600).unwrap(), line.as_bytes()).unwrap();
        }
        assert_eq!(std::fs::read_to_string(&log).unwrap(), "a\nb\n");
        assert_mode(&log, 0o600);
    }

    /// A Windows append handle has no FILE_WRITE_DATA, so it cannot truncate: `set_len` cuts
    /// a torn tail through `open_append`'s and std's append handles alike, while another
    /// handle reads the file (Local History's index), and appends still land at the new end.
    #[test]
    fn set_len_truncates_append_handles() {
        use std::io::Write;
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("index");
        let mut f = open_append(&p, 0o600).unwrap();
        f.write_all(b"one\ntorn").unwrap();
        let reader = File::open(&p).unwrap();
        set_len(&f, 4).unwrap();
        f.write_all(b"two\n").unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "one\ntwo\n");

        let mut g = std::fs::OpenOptions::new().append(true).open(&p).unwrap();
        g.write_all(b"torn").unwrap();
        #[cfg(windows)]
        assert!(g.set_len(12).is_err(), "std's append handle cannot truncate on Windows");
        set_len(&g, 8).unwrap();
        g.write_all(b"three\n").unwrap();
        drop(reader);
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "one\ntwo\nthree\n");
        assert_mode(&p, 0o600);
    }

    #[test]
    fn private_dirs_are_created_with_their_parents() {
        let d = tempfile::tempdir().unwrap();
        let deep = d.path().join("a/b/c");
        create_dir_private(&deep).unwrap();
        for p in [d.path().join("a"), d.path().join("a/b"), deep.clone()] {
            assert_mode(&p, 0o700);
        }
        create_dir_private(&deep).unwrap();
        std::fs::write(deep.join("f"), "x").unwrap();
        assert!(create_dir_private(&deep.join("f")).is_err());
    }

    #[test]
    fn privacy_is_reported_and_fixed() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("token");
        std::fs::write(&p, "t").unwrap();
        expose(&p, 0o644);
        let why = match privacy(&p).unwrap() {
            Privacy::Exposed(why) => why,
            Privacy::Private => panic!("{} should be readable by others", p.display()),
        };
        assert!(if cfg!(unix) { why == "mode 644" } else { why.contains("can read it") }, "{why}");
        assert_eq!(describe(&p).unwrap(), if cfg!(unix) { "644" } else { "shared" });
        apply(&p, 0o600).unwrap();
        assert_mode(&p, 0o600);
        assert_eq!(describe(&p).unwrap(), if cfg!(unix) { "600" } else { "private" });
        assert!(privacy(&d.path().join("missing")).is_err());
    }

    #[test]
    fn replacements_keep_the_target_permissions() {
        let d = tempfile::tempdir().unwrap();
        let target = d.path().join("shared.txt");
        std::fs::write(&target, "old").unwrap();
        expose(&target, 0o664);
        let tmp = d.path().join(".shared.txt.tmp");
        let mut f = create_replacement(&tmp, &target, Some(0o600)).unwrap();
        std::io::Write::write_all(&mut f, b"new").unwrap();
        drop(f);
        assert!(privacy(&tmp).unwrap().is_exposed());
        rename_into_place(&tmp, &target).unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "new");
        assert!(!tmp.exists());

        // A private target stays private; a new file gets the mode.
        let private = d.path().join("private.txt");
        std::fs::write(&private, "old").unwrap();
        apply(&private, 0o600).unwrap();
        let tmp = d.path().join(".private.txt.tmp");
        drop(create_replacement(&tmp, &private, None).unwrap());
        assert_mode(&tmp, 0o600);
        let fresh = d.path().join(".fresh.tmp");
        drop(create_replacement(&fresh, &d.path().join("fresh"), Some(0o600)).unwrap());
        assert_mode(&fresh, 0o600);
        assert_eq!(create_replacement(&fresh, &target, None).unwrap_err().kind(), io::ErrorKind::AlreadyExists);
    }

    #[test]
    fn new_files_never_go_through_a_symlink() {
        let d = tempfile::tempdir().unwrap();
        let (link, target) = (d.path().join("link"), d.path().join("target"));
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, &link).unwrap();
        // Windows: creating a symlink needs Developer Mode or an administrator.
        #[cfg(windows)]
        {
            if std::os::windows::fs::symlink_file(&target, &link).is_err() {
                return;
            }
        }
        for nofollow in [false, true] {
            assert_eq!(open_new(&link, 0o600, nofollow).unwrap_err().kind(), io::ErrorKind::AlreadyExists);
        }
        let err = create_replacement(&link, &d.path().join("file"), Some(0o600)).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        assert!(!target.exists(), "nothing was created at the link's target");
    }

    #[test]
    fn apply_to_changes_an_open_file() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("f");
        let f = open_new(&p, 0o644, false).unwrap();
        apply_to(&f, 0o600).unwrap();
        assert_mode(&p, 0o600);
        assert_eq!(mode(&f.metadata().unwrap()).map(|m| m & 0o777), if cfg!(unix) { Some(0o600) } else { None });
    }

    /// The DACLs themselves: protected and owner-only at creation, inherited inside a private
    /// directory, and copied (own entries and protection) by `create_replacement`.
    #[cfg(windows)]
    #[test]
    fn dacls_are_set_at_creation() {
        use windows_sys::Win32::Security::{DACL_SECURITY_INFORMATION, INHERITED_ACE};
        let d = tempfile::tempdir().unwrap();
        let entries = |p: &Path| {
            let s = imp::Security::of(p, DACL_SECURITY_INFORMATION).unwrap();
            (s.protected(), imp::aces(s.dacl).iter().map(|a| a.flags & INHERITED_ACE != 0).collect::<Vec<_>>())
        };
        let f = d.path().join("f");
        drop(open_new(&f, 0o600, false).unwrap());
        assert_eq!(entries(&f), (true, vec![false, false]), "user and SYSTEM, explicit");

        let dir = d.path().join("private");
        create_dir_private(&dir).unwrap();
        std::fs::write(dir.join("inside"), "x").unwrap();
        assert_eq!(entries(&dir.join("inside")), (false, vec![true, true]), "inherited from the directory");
        assert_mode(&dir.join("inside"), 0o600);

        let shared = d.path().join("shared");
        std::fs::write(&shared, "x").unwrap();
        expose(&shared, 0o644);
        let tmp = d.path().join("shared.tmp");
        drop(create_replacement(&tmp, &shared, Some(0o600)).unwrap());
        assert_eq!(entries(&tmp).1.iter().filter(|inherited| !**inherited).count(), 2, "user and Everyone copied");
        assert!(privacy(&tmp).unwrap().is_exposed());

        let long = d.path().join("x".repeat(120)).join("y".repeat(120));
        create_dir_private(&long).unwrap();
        drop(open_new(&long.join("z".repeat(40)), 0o600, false).unwrap());
        assert_mode(&long.join("z".repeat(40)), 0o600);
    }
}
