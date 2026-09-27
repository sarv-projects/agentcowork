//! Length-prefixed framing: `[u32 LE length][JSON payload]`.
//!
//! The length prefix is written as 4 bytes little-endian, then exactly that
//! many payload bytes. Payloads larger than [`MAX_FRAME_LEN`] are rejected
//! by the transport — the app layer must use `ref:` handles instead (C10).

use std::io::{self, Read, Write};

/// Hard cap on a single frame's payload (16 MiB). Larger objects travel as
/// `ref:` handles (spec C10), never as inline JSON.
pub const MAX_FRAME_LEN: u32 = 16 * 1024 * 1024;

/// Encode a JSON payload into a framed byte buffer.
pub fn encode(payload: &[u8]) -> Vec<u8> {
    let len = u32::try_from(payload.len()).expect("payload exceeds u32");
    debug_assert!(len <= MAX_FRAME_LEN, "payload exceeds MAX_FRAME_LEN");
    let mut out = Vec::with_capacity(4 + payload.len());
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(payload);
    out
}

/// Decode exactly one frame from `reader`, returning `None` on clean EOF at
/// a frame boundary. Rejects oversized frames with [`FrameError::TooLarge`].
pub fn decode<R: Read>(reader: &mut R) -> Result<Option<Vec<u8>>, FrameError> {
    let mut header = [0u8; 4];
    match reader.read_exact(&mut header) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(FrameError::Io(e)),
    }
    let len = u32::from_le_bytes(header);
    if len > MAX_FRAME_LEN {
        return Err(FrameError::TooLarge(len));
    }
    let mut buf = vec![0u8; len as usize];
    reader.read_exact(&mut buf)?;
    Ok(Some(buf))
}

/// Read all complete frames from a byte slice (used for tests / buffers).
pub fn decode_all(bytes: &[u8]) -> Result<Vec<Vec<u8>>, FrameError> {
    let mut cursor = io::Cursor::new(bytes);
    let mut out = Vec::new();
    while let Some(frame) = decode(&mut cursor)? {
        out.push(frame);
    }
    Ok(out)
}

/// Write a framed payload to `writer` (appends the 4-byte prefix + payload).
pub fn write_frame<W: Write>(writer: &mut W, payload: &[u8]) -> io::Result<()> {
    writer.write_all(&encode(payload))
}

#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    #[error("io error: {0}")]
    Io(#[from] io::Error),
    #[error("frame too large: {0} bytes (max {MAX_FRAME_LEN})")]
    TooLarge(u32),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_single_frame() {
        let payload = br#"{"jsonrpc":"2.0","method":"echo","params":[1,2,3]}"#;
        let framed = encode(payload);
        assert_eq!(&framed[..4], &(payload.len() as u32).to_le_bytes());
        let decoded = decode_all(&framed).expect("decode ok");
        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded[0], payload);
    }

    #[test]
    fn concatenated_frames_decode_in_order() {
        let a = encode(br#"{"id":1}"#);
        let b = encode(br#"{"id":2}"#);
        let mut joined = a.clone();
        joined.extend_from_slice(&b);
        let decoded = decode_all(&joined).expect("decode ok");
        assert_eq!(
            decoded,
            vec![br#"{"id":1}"#.to_vec(), br#"{"id":2}"#.to_vec()]
        );
    }

    #[test]
    fn partial_buffer_returns_io_eof_not_garbage() {
        // A half-received frame: the header arrived but the payload is
        // truncated. The decoder must surface an explicit EOF-style error
        // (the buffered reader only calls decode() once a full frame is
        // available, so this is the fail-safe path), never a bogus frame.
        let framed = encode(br#"{"x":1}"#);
        let partial = &framed[..6]; // 4 header bytes + 2 payload bytes
        let mut cursor = io::Cursor::new(partial);
        match decode(&mut cursor) {
            Err(FrameError::Io(e)) => {
                assert_eq!(e.kind(), io::ErrorKind::UnexpectedEof);
            }
            other => panic!("expected UnexpectedEof error, got {other:?}"),
        }
    }

    #[test]
    fn oversized_frame_rejected() {
        let too_big = MAX_FRAME_LEN + 1;
        let mut buf = Vec::new();
        buf.extend_from_slice(&too_big.to_le_bytes());
        buf.extend_from_slice(&[0u8; 8]);
        let mut cursor = io::Cursor::new(buf);
        match decode(&mut cursor) {
            Err(FrameError::TooLarge(n)) => assert_eq!(n, too_big),
            other => panic!("expected TooLarge, got {other:?}"),
        }
    }

    #[test]
    fn empty_payload_frame() {
        let framed = encode(b"");
        assert_eq!(decode_all(&framed).unwrap(), vec![Vec::<u8>::new()]);
    }
}
