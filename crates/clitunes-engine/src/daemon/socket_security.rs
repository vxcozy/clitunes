use std::os::unix::net::UnixStream;

use super::peercred::{my_uid, peer_cred};

#[derive(Debug)]
pub enum AcceptGuard {
    Allowed(UnixStream),
    Rejected { peer_uid: u32, our_uid: u32 },
    PeercredFailed(std::io::Error),
}

pub fn check_peer(stream: UnixStream) -> AcceptGuard {
    let our_uid = my_uid();
    match peer_cred(&stream) {
        Ok(cred) => {
            if cred.uid == our_uid {
                AcceptGuard::Allowed(stream)
            } else {
                tracing::warn!(
                    peer_uid = cred.uid,
                    peer_pid = cred.pid,
                    our_uid = our_uid,
                    "rejected connection from different UID"
                );
                drop(stream);
                AcceptGuard::Rejected {
                    peer_uid: cred.uid,
                    our_uid,
                }
            }
        }
        Err(e) => {
            tracing::warn!(
                error = %e,
                "peercred query failed; denying connection"
            );
            drop(stream);
            AcceptGuard::PeercredFailed(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixStream;

    #[test]
    fn same_uid_is_allowed() {
        let (a, _b) = UnixStream::pair().unwrap();
        match check_peer(a) {
            AcceptGuard::Allowed(_) => {}
            other => panic!("expected Allowed, got {other:?}"),
        }
    }
}
