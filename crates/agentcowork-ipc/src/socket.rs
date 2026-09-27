//! UNIX-domain socket transport (J16) — preferred over TCP for local IPC.
//!
//! Zero port collisions (the kernel owns the path namespace), no firewall
//! prompts, and no loopback interface dependency. The server speaks the same
//! length-prefixed framing as stdio — `[u32 LE len][JSON payload]` — so both
//! transports share one wire contract and one decoder.
//!
//! Unix-only by nature; gated with `#[cfg(unix)]`.

use std::io;
use std::net::Shutdown;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};

use crate::frame::{self, FrameError};

/// Default socket file name inside the data dir.
pub const SOCKET_FILE_NAME: &str = "coordinator.sock";

/// The unix socket path for a data dir: `<data_dir>/coordinator.sock`.
pub fn socket_path(data_dir: &Path) -> PathBuf {
    data_dir.join(SOCKET_FILE_NAME)
}

/// A framed UNIX-socket server. Connections are served sequentially on the
/// accepting thread — sufficient for a local control channel (the coordinator
/// is the only peer). Concurrency can be layered on later if needed.
pub struct UnixFrameServer {
    listener: UnixListener,
    path: PathBuf,
}

impl UnixFrameServer {
    /// Bind `path`, removing a stale socket file first: a dead process leaves
    /// one behind, which would otherwise make `bind` fail with EADDRINUSE.
    pub fn bind(path: &Path) -> io::Result<Self> {
        if path.exists() {
            let _ = std::fs::remove_file(path);
        }
        let listener = UnixListener::bind(path)?;
        Ok(Self {
            listener,
            path: path.to_path_buf(),
        })
    }

    pub fn local_path(&self) -> &Path {
        &self.path
    }

    /// Accept a single connection.
    pub fn accept(&self) -> io::Result<UnixStream> {
        let (stream, _) = self.listener.accept()?;
        Ok(stream)
    }

    /// Serve a handler over one connection: read frames, call `handler` with
    /// each payload; a `Some` return is written back as a frame. Returns on
    /// clean EOF (peer closed at a frame boundary) or an IO/framing error.
    pub fn serve_connection<F>(
        &self,
        mut stream: UnixStream,
        mut handler: F,
    ) -> Result<(), SocketError>
    where
        F: FnMut(Vec<u8>) -> Option<Vec<u8>>,
    {
        loop {
            match frame::decode(&mut stream) {
                Ok(Some(payload)) => {
                    if let Some(reply) = handler(payload) {
                        frame::write_frame(&mut stream, &reply)?;
                    }
                }
                Ok(None) => break,
                Err(FrameError::Io(e)) if e.kind() == io::ErrorKind::UnexpectedEof => break,
                Err(e) => return Err(SocketError::Frame(e)),
            }
        }
        let _ = stream.shutdown(Shutdown::Both);
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SocketError {
    #[error("io error: {0}")]
    Io(#[from] io::Error),
    #[error("framing error: {0}")]
    Frame(#[from] FrameError),
}

/// Connect to a unix frame server and run one framed request/response.
pub fn request(path: &Path, payload: &[u8]) -> Result<Vec<u8>, SocketError> {
    let mut stream = UnixStream::connect(path)?;
    frame::write_frame(&mut stream, payload)?;
    let reply = frame::decode(&mut stream)?.ok_or_else(|| {
        SocketError::Io(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "server closed without a reply",
        ))
    })?;
    Ok(reply)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_socket(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("agentcowork-{name}-{}.sock", std::process::id()))
    }

    #[test]
    fn echo_roundtrip_over_real_socket() {
        let path = tmp_socket("echo");
        let _ = std::fs::remove_file(&path);
        let server = UnixFrameServer::bind(&path).expect("bind");
        let thread_path = path.clone();
        let handle = std::thread::spawn(move || {
            let stream = server.accept().expect("accept");
            server.serve_connection(stream, Some).expect("serve");
        });

        let payload = br#"{"jsonrpc":"2.0","id":1,"method":"session/ping"}"#;
        let reply = request(&thread_path, payload).expect("request");
        assert_eq!(reply, payload);
        handle.join().unwrap();
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn two_sequential_clients_served() {
        let path = tmp_socket("seq");
        let _ = std::fs::remove_file(&path);
        let server = UnixFrameServer::bind(&path).expect("bind");
        let thread_path = path.clone();
        let handle = std::thread::spawn(move || {
            for _ in 0..2 {
                let stream = server.accept().expect("accept");
                server.serve_connection(stream, Some).expect("serve");
            }
        });

        for i in 0..2u32 {
            let payload = format!(r#"{{"id":{i}}}"#);
            let reply = request(&thread_path, payload.as_bytes()).expect("request");
            assert_eq!(reply, payload.as_bytes());
        }
        handle.join().unwrap();
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn stale_socket_is_replaced_on_rebind() {
        let path = tmp_socket("stale");
        let _ = std::fs::remove_file(&path);
        // Simulate a stale socket left by a dead process.
        std::fs::write(&path, b"stale").expect("write stale socket file");
        let server = UnixFrameServer::bind(&path).expect("rebind must succeed");
        assert!(path.exists());
        let thread_path = path.clone();
        let handle = std::thread::spawn(move || {
            let stream = server.accept().expect("accept");
            let _ = server.serve_connection(stream, Some);
        });
        let reply = request(&thread_path, b"ping").expect("request");
        assert_eq!(reply, b"ping");
        handle.join().unwrap();
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn ipc_latency_below_2ms_per_crossing() {
        // P0.5 benchmark: a full request→response crossing through a real OS
        // socket pair (kernel round trip + framing) — the transport the app
        // actually uses. Print the measured average; assert it stays under the
        // 2 ms budget (≈100× the expected kernel round trip, so never flaky).
        const ROUND_TRIPS: u32 = 2_000;
        const BUDGET_NS: u128 = 2_000_000; // 2 ms

        let (mut a, mut b) = UnixStream::pair().expect("socketpair");
        let server = std::thread::spawn(move || {
            let mut buf = Vec::new();
            while let Some(frame) = frame::decode(&mut b).expect("decode") {
                buf.clear();
                buf.extend_from_slice(&frame);
                frame::write_frame(&mut b, &buf).expect("write");
            }
        });

        let payload = br#"{"jsonrpc":"2.0","id":1,"method":"session/ping"}"#;
        for _ in 0..50 {
            // Warm-up (page faults, allocator, scheduler noise).
            frame::write_frame(&mut a, payload).expect("write");
            frame::decode(&mut a).expect("decode").expect("frame");
        }

        let start = std::time::Instant::now();
        for _ in 0..ROUND_TRIPS {
            frame::write_frame(&mut a, payload).expect("write");
            frame::decode(&mut a).expect("decode").expect("frame");
        }
        let elapsed = start.elapsed();

        let avg_ns = elapsed.as_nanos() / ROUND_TRIPS as u128;
        let avg_us = avg_ns / 1_000;
        eprintln!(
            "ipc-latency: {ROUND_TRIPS} round trips in {elapsed:?} — avg {avg_us} µs/crossing"
        );
        assert!(
            avg_ns <= BUDGET_NS,
            "avg {avg_us}µs/crossing exceeds 2 ms budget"
        );
        let _ = a.shutdown(Shutdown::Both);
        server.join().unwrap();
    }

    #[test]
    fn request_to_nonexistent_socket_fails() {
        let path = tmp_socket("nope");
        let _ = std::fs::remove_file(&path);
        assert!(request(&path, b"ping").is_err());
    }
}
