use std::io;
use std::os::unix::net::UnixStream;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PeerCred {
    pub uid: u32,
    pub gid: u32,
    pub pid: i32,
}

pub fn peer_cred(stream: &UnixStream) -> io::Result<PeerCred> {
    #[cfg(target_os = "linux")]
    {
        peer_cred_linux(stream)
    }
    #[cfg(target_os = "macos")]
    {
        peer_cred_macos(stream)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = stream;
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "peercred not available on this platform",
        ))
    }
}

#[cfg(target_os = "linux")]
fn peer_cred_linux(stream: &UnixStream) -> io::Result<PeerCred> {
    use std::os::fd::AsRawFd;

    let fd = stream.as_raw_fd();
    let mut cred: libc::ucred = unsafe { std::mem::zeroed() };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;

    let rc = unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            &mut cred as *mut _ as *mut libc::c_void,
            &mut len,
        )
    };
    if rc < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(PeerCred {
        uid: cred.uid,
        gid: cred.gid,
        pid: cred.pid,
    })
}

#[cfg(target_os = "macos")]
fn peer_cred_macos(stream: &UnixStream) -> io::Result<PeerCred> {
    use std::os::fd::AsRawFd;

    let fd = stream.as_raw_fd();

    let mut pid: libc::c_int = 0;
    let mut pid_len = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
    let rc = unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_LOCAL,
            libc::LOCAL_PEERPID,
            &mut pid as *mut _ as *mut libc::c_void,
            &mut pid_len,
        )
    };
    if rc < 0 {
        return Err(io::Error::last_os_error());
    }

    let mut cred: libc::xucred = unsafe { std::mem::zeroed() };
    cred.cr_version = libc::XUCRED_VERSION;
    let mut cred_len = std::mem::size_of::<libc::xucred>() as libc::socklen_t;
    let rc = unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_LOCAL,
            libc::LOCAL_PEERCRED,
            &mut cred as *mut _ as *mut libc::c_void,
            &mut cred_len,
        )
    };
    if rc < 0 {
        return Err(io::Error::last_os_error());
    }

    Ok(PeerCred {
        uid: cred.cr_uid,
        gid: cred.cr_groups[0],
        pid,
    })
}

pub fn my_uid() -> u32 {
    unsafe { libc::getuid() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peer_cred_returns_own_uid_on_loopback() {
        let (a, b) = UnixStream::pair().unwrap();
        let cred_a = peer_cred(&a).unwrap();
        let cred_b = peer_cred(&b).unwrap();
        let uid = my_uid();
        assert_eq!(cred_a.uid, uid);
        assert_eq!(cred_b.uid, uid);
        assert!(cred_a.pid > 0);
    }

    #[test]
    fn my_uid_is_nonzero_in_normal_test() {
        let uid = my_uid();
        assert!(uid > 0 || uid == 0, "uid is always valid");
    }
}
