//! The socket round trip: listen, send_all, scope, stale sockets and the
//! directory checks. Socket waits are bounded; negative checks wait
//! 300 ms, far longer than a local socket write.
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use debrief_follow as f;
use tempfile::TempDir;

/// A short socket dir: Unix socket paths are capped near 104 bytes.
fn sock_dir() -> (TempDir, PathBuf) {
    let base = tempfile::Builder::new()
        .prefix("f")
        .tempdir_in("/tmp")
        .unwrap();
    let dir = base.path().join("follow-test");
    (base, dir)
}

#[test]
fn a_sent_line_reaches_the_listener_in_scope_only() {
    let (_base, dir) = sock_dir();
    let (tx, rx) = mpsc::channel();
    let listener = f::listen(&dir, "t1w2", "debrief", move |p, l| {
        let _ = tx.send((p, l));
    })
    .unwrap();
    assert!(listener.path().exists());

    // Another window's sender doesn't reach it (negative window: 300 ms
    // is far longer than a local socket write).
    f::send_all(&dir, "t1w3", 7, Path::new("/n/a.md"));
    assert!(rx.recv_timeout(Duration::from_millis(300)).is_err());

    f::send_all(&dir, "t1w2", 7, Path::new("/n/a b.md"));
    let got = rx.recv_timeout(Duration::from_secs(2)).expect("delivered");
    assert_eq!(got, (PathBuf::from("/n/a b.md"), 7));

    let sock = listener.path().to_path_buf();
    drop(listener);
    assert!(!sock.exists(), "dropping the listener removes the socket");
}

#[test]
fn a_dead_socket_is_removed_by_the_next_sender() {
    let (_base, dir) = sock_dir();
    f::ensure_dir(&dir).unwrap();
    let stale = dir.join(f::socket_name("any", "ramble", 1));
    // Bind and drop: the file stays, nothing listens.
    drop(std::os::unix::net::UnixListener::bind(&stale).unwrap());
    assert!(stale.exists());
    f::send_all(&dir, "any", 1, Path::new("/x.md"));
    assert!(!stale.exists());
}

#[test]
fn a_socket_dir_others_can_open_is_refused() {
    use std::os::unix::fs::PermissionsExt;
    let (_base, dir) = sock_dir();
    std::fs::create_dir(&dir).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    let err = f::listen(&dir, "any", "debrief", |_, _| {}).unwrap_err();
    assert!(err.to_string().contains("other users"), "{err}");
}

#[test]
fn a_client_holding_its_connection_open_does_not_stall_others() {
    use std::io::Write;
    let (_base, dir) = sock_dir();
    let (tx, rx) = mpsc::channel();
    let listener = f::listen(&dir, "any", "debrief", move |p, l| {
        let _ = tx.send((p, l));
    })
    .unwrap();
    // Keeps writing well inside the per-read timeout, never closes.
    let sock = listener.path().to_path_buf();
    let hog = std::thread::spawn(move || {
        let mut s = std::os::unix::net::UnixStream::connect(&sock).unwrap();
        for _ in 0..40 {
            if s.write_all(b"1 /hog.md\n").is_err() {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    });
    std::thread::sleep(Duration::from_millis(50));
    f::send_all(&dir, "any", 7, Path::new("/n/a.md"));
    // The hog gets at most its own deadline (500 ms); the sender must be
    // served well before the hog's 4 s of writes end.
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut got = None;
    while let Ok(m) = rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
        if m.0 == Path::new("/n/a.md") {
            got = Some(m);
            break;
        }
    }
    assert_eq!(got, Some((PathBuf::from("/n/a.md"), 7)));
    hog.join().unwrap();
}

#[test]
fn senders_skip_a_socket_dir_others_can_open_and_non_sockets() {
    use std::os::unix::fs::PermissionsExt;
    let (_base, dir) = sock_dir();
    let (tx, rx) = mpsc::channel();
    let listener = f::listen(&dir, "any", "debrief", move |p, l| {
        let _ = tx.send((p, l));
    })
    .unwrap();
    // A plain file named like a socket is never touched.
    let plain = dir.join(f::socket_name("any", "ramble", 2));
    std::fs::write(&plain, "keep").unwrap();
    // Open the dir to others: as if someone else had made it.
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o777)).unwrap();
    f::send_all(&dir, "any", 3, Path::new("/n/a.md"));
    // Negative window: a local socket write lands in well under 300 ms.
    assert!(
        rx.recv_timeout(Duration::from_millis(300)).is_err(),
        "sent into a dir others can open"
    );
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    f::send_all(&dir, "any", 3, Path::new("/n/a.md"));
    assert!(rx.recv_timeout(Duration::from_secs(2)).is_ok());
    assert!(plain.exists(), "a non-socket *.sock was removed");
    drop(listener);
}
