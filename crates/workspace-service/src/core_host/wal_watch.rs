//! A filesystem wake hints that committed SQLite state may have changed.
use std::{io, path::Path};

#[cfg(target_os = "linux")]
use std::{
    ffi::CString,
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    os::unix::ffi::OsStrExt,
};
#[cfg(target_os = "linux")]
use tokio::io::{Interest, unix::AsyncFd};

#[cfg(target_os = "linux")]
pub(super) struct Watch(AsyncFd<OwnedFd>);

#[cfg(not(target_os = "linux"))]
pub(super) struct Watch;

#[cfg(target_os = "linux")]
impl Watch {
    pub(super) fn new(path: &Path) -> io::Result<Self> {
        let path = CString::new(path.as_os_str().as_bytes())
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
        let fd = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // The directory's watch survives a checkpoint or replacement of the WAL inode.
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };
        let mask = libc::IN_MODIFY
            | libc::IN_CREATE
            | libc::IN_MOVED_TO
            | libc::IN_MOVED_FROM
            | libc::IN_DELETE
            | libc::IN_CLOSE_WRITE
            | libc::IN_MOVE_SELF
            | libc::IN_DELETE_SELF
            | libc::IN_ONLYDIR
            | libc::IN_DONT_FOLLOW;
        if unsafe { libc::inotify_add_watch(fd.as_raw_fd(), path.as_ptr(), mask) } < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: AsyncFd owns this exclusively held, nonblocking descriptor.
        Ok(Self(unsafe {
            AsyncFd::register_with_interest(fd, Interest::READABLE)
        }?))
    }

    async fn changed(&self) -> io::Result<()> {
        // Each readiness consumes a batch. The reader's 25 ms cooldown also bounds
        // repeated wakeups if a writer keeps changing the WAL faster than we read it.
        let mut events = [0u8; 4096];
        loop {
            let mut ready = self.0.readable().await?;
            match ready.try_io(|fd| {
                let size = unsafe {
                    libc::read(
                        fd.get_ref().as_raw_fd(),
                        events.as_mut_ptr().cast(),
                        events.len(),
                    )
                };
                if size < 0 {
                    return Err(io::Error::last_os_error());
                }
                if size == 0 {
                    return Err(io::ErrorKind::UnexpectedEof.into());
                }
                wal_changed(&events[..size as usize])
            }) {
                Ok(Ok(false)) => tokio::task::yield_now().await,
                Err(_) => continue,
                Ok(result) => return result.map(|_| ()),
            }
        }
    }
}

#[cfg(target_os = "linux")]
fn wal_changed(mut bytes: &[u8]) -> io::Result<bool> {
    const HEADER: usize = std::mem::size_of::<libc::inotify_event>();
    const LOST: u32 =
        libc::IN_IGNORED | libc::IN_UNMOUNT | libc::IN_MOVE_SELF | libc::IN_DELETE_SELF;
    const REPLACED: u32 =
        libc::IN_CREATE | libc::IN_DELETE | libc::IN_MOVED_FROM | libc::IN_MOVED_TO;
    let mut changed = false;
    while !bytes.is_empty() {
        let header = bytes.get(..HEADER).ok_or(io::ErrorKind::InvalidData)?;
        let mask = u32::from_ne_bytes(header[4..8].try_into().unwrap());
        let len = u32::from_ne_bytes(header[12..16].try_into().unwrap()) as usize;
        let (name, remaining) = bytes[HEADER..]
            .split_at_checked(len)
            .ok_or(io::ErrorKind::InvalidData)?;
        if mask & LOST != 0 {
            return Err(io::ErrorKind::NotFound.into());
        }
        let name = name.split(|byte| *byte == 0).next().unwrap_or_default();
        changed |= name == b"cortex.sqlite3-wal"
            || (name == b"cortex.sqlite3" && mask & REPLACED != 0)
            || mask & libc::IN_Q_OVERFLOW != 0;
        bytes = remaining;
    }
    Ok(changed)
}

#[cfg(not(target_os = "linux"))]
impl Watch {
    pub(super) fn new(_path: &Path) -> io::Result<Self> {
        Err(io::ErrorKind::Unsupported.into())
    }
}

pub(super) async fn wait(watch: Option<&Watch>) -> io::Result<()> {
    #[cfg(target_os = "linux")]
    if let Some(watch) = watch {
        return watch.changed().await;
    }
    let _ = watch;
    std::future::pending().await
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    #[test]
    fn reports_an_overflow_as_a_poll_hint() {
        let mut event = [0; std::mem::size_of::<libc::inotify_event>()];
        event[4..8].copy_from_slice(&libc::IN_Q_OVERFLOW.to_ne_bytes());
        assert!(wal_changed(&event).unwrap());
        assert!(wal_changed(&event[..7]).is_err());
    }
}
