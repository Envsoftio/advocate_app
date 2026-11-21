use std::{fs::File, io};

/// Identity and change records from an open file descriptor. Access times
/// are excluded because reading the protected file may update them.
#[derive(PartialEq, Eq)]
pub(in crate::matters) struct FileVersion {
    device: u64,
    inode: u64,
    bytes: u64,
    modified: (i64, i64),
    changed: (i64, i64),
}

impl FileVersion {
    #[cfg(target_os = "macos")]
    pub fn read(file: &File) -> io::Result<Self> {
        use std::{ffi::c_char, mem::MaybeUninit, os::fd::AsRawFd, os::unix::fs::MetadataExt};
        let mut filesystem = MaybeUninit::<libc::statfs>::uninit();
        // SAFETY: The descriptor is borrowed from a live File and fstatfs
        // initializes the output on success.
        let filesystem = unsafe {
            if libc::fstatfs(file.as_raw_fd(), filesystem.as_mut_ptr()) != 0 {
                return Err(io::Error::last_os_error());
            }
            filesystem.assume_init()
        };
        // Other filesystems can report simulated or writable change times.
        if filesystem.f_flags & libc::MNT_LOCAL as u32 == 0
            || filesystem.f_fstypename[..5] != b"apfs\0".map(|byte| byte as c_char)
        {
            return Err(io::ErrorKind::Unsupported.into());
        }
        let info = file.metadata()?;
        Ok(Self {
            device: info.dev(),
            inode: info.ino(),
            bytes: info.len(),
            modified: (info.mtime(), info.mtime_nsec()),
            changed: (info.ctime(), info.ctime_nsec()),
        })
    }

    #[cfg(not(target_os = "macos"))]
    pub fn read(_file: &File) -> io::Result<Self> {
        Err(io::ErrorKind::Unsupported.into())
    }
}
