//! ACP stdio transport framing (doc 45 §1.5): **newline-delimited JSON-RPC**
//! over stdin/stdout. One JSON object per line; `\n` terminates each message.
//! (Note: ACP uses newline-delimited frames, not the `Content-Length` framing
//! of LSP — so this is a distinct, simpler codec than `agentcowork-codeintel`.)

use std::io;

/// Hard cap for one ACP JSON payload, excluding its terminating newline.
///
/// The cap applies to complete and partial frames. A peer therefore cannot grow
/// the reader buffer forever by withholding `\n`, and a single oversized line is
/// rejected before it is converted to an owned `String`.
pub const MAX_ACP_FRAME_BYTES: usize = 8 * 1024 * 1024;
const MAX_FRAMES_PER_DECODE: usize = 256;

fn oversized_frame_error() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("ACP frame exceeds the {MAX_ACP_FRAME_BYTES}-byte limit"),
    )
}

/// Serialize one message into a newline-terminated frame without applying the
/// transport byte cap.
///
/// Session and process transports use [`try_encode_message`]. This compatibility
/// helper remains infallible for existing fixture builders; production writes
/// must use the checked form so an oversized or frame-injecting value cannot
/// reach the child pipe.
pub fn encode_message(json: &str) -> String {
    let mut out = String::with_capacity(json.len() + 1);
    out.push_str(json);
    // ACP expects a single trailing newline; strip any internal trailing
    // newlines so we never emit a blank extra frame.
    while out.ends_with('\n') {
        out.pop();
    }
    out.push('\n');
    out
}

/// Serialize one bounded, single-frame ACP message.
pub fn try_encode_message(json: &str) -> io::Result<String> {
    let payload_len = json.trim_end_matches('\n').len();
    if payload_len > MAX_ACP_FRAME_BYTES {
        return Err(oversized_frame_error());
    }
    if json[..payload_len]
        .bytes()
        .any(|byte| matches!(byte, b'\n' | b'\r'))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "ACP frame contains an embedded newline",
        ));
    }
    Ok(encode_message(json))
}

/// Extract complete newline-delimited messages from `buf`, leaving any
/// partial (no trailing newline) data in place. Returns each complete message
/// (without the newline).
pub fn decode_messages(buf: &mut Vec<u8>) -> io::Result<Vec<String>> {
    // Validate the complete buffered extent before yielding any frame. If a
    // later line is poisoned, the reader will fail the whole transport instead
    // of exposing an apparently usable prefix from a desynchronized stream.
    if buf.len() > MAX_ACP_FRAME_BYTES && !buf.contains(&b'\n') {
        return Err(oversized_frame_error());
    }

    let mut out = Vec::new();
    let mut start = 0usize;
    while let Some(pos) = find_newline(buf, start) {
        let line = &buf[start..pos];
        if line.len() > MAX_ACP_FRAME_BYTES {
            return Err(oversized_frame_error());
        }
        let s = std::str::from_utf8(line)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?
            .to_string();
        // Skip blank lines (some agents emit a trailing extra newline).
        if !s.trim().is_empty() {
            out.push(s);
            if out.len() >= MAX_FRAMES_PER_DECODE {
                start = pos + 1;
                break;
            }
        }
        start = pos + 1;
    }
    if start > 0 {
        buf.drain(..start);
    }

    // The final line is partial. It is still bounded even though it has no
    // newline yet; this is the important no-newline growth guard.
    if buf.len() > MAX_ACP_FRAME_BYTES {
        return Err(oversized_frame_error());
    }
    Ok(out)
}

/// Validate a buffer at EOF. A non-empty unterminated frame is a truncated ACP
/// message, not a clean end-of-stream.
pub fn finish_decode(buf: &[u8]) -> io::Result<()> {
    if buf.is_empty() {
        return Ok(());
    }
    if buf.len() > MAX_ACP_FRAME_BYTES {
        return Err(oversized_frame_error());
    }
    Err(io::Error::new(
        io::ErrorKind::UnexpectedEof,
        "ACP stream ended with a partial frame",
    ))
}

fn find_newline(buf: &[u8], from: usize) -> Option<usize> {
    buf[from..]
        .iter()
        .position(|&b| b == b'\n')
        .map(|i| from + i)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_appends_single_newline() {
        assert_eq!(encode_message("{}"), "{}\n");
        assert_eq!(encode_message("{\"a\":1}"), "{\"a\":1}\n");
        assert_eq!(encode_message("{}"), "{}\n"); // no double newline
    }

    #[test]
    fn checked_encode_rejects_oversized_and_multiline_frames() {
        let oversized = "x".repeat(MAX_ACP_FRAME_BYTES + 1);
        assert_eq!(
            try_encode_message(&oversized).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(
            try_encode_message("{}\n{}").unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
    }

    #[test]
    fn decode_splits_complete_lines_and_keeps_partial() {
        let mut buf = b"{\"a\":1}\n{\"b\":2}\n{\"c\":".to_vec();
        let msgs = decode_messages(&mut buf).unwrap();
        assert_eq!(msgs, vec!["{\"a\":1}", "{\"b\":2}"]);
        // Partial line remains for the next chunk.
        assert_eq!(buf, b"{\"c\":");

        // Feeding the rest completes the message.
        buf.extend_from_slice(b"3}\n");
        let msgs = decode_messages(&mut buf).unwrap();
        assert_eq!(msgs, vec!["{\"c\":3}"]);
        assert!(buf.is_empty());
    }

    #[test]
    fn decode_skips_blank_lines() {
        let mut buf = b"{\"a\":1}\n\n{\"b\":2}\n".to_vec();
        let msgs = decode_messages(&mut buf).unwrap();
        assert_eq!(msgs, vec!["{\"a\":1}", "{\"b\":2}"]);
    }

    #[test]
    fn decode_no_newline_returns_empty() {
        let mut buf = b"{\"partial\"".to_vec();
        let msgs = decode_messages(&mut buf).unwrap();
        assert!(msgs.is_empty());
        assert_eq!(buf, b"{\"partial\"");
        assert_eq!(
            finish_decode(&buf).unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
    }

    #[test]
    fn decode_rejects_oversized_complete_and_partial_frames() {
        let mut complete = vec![b'x'; MAX_ACP_FRAME_BYTES + 1];
        complete.push(b'\n');
        assert_eq!(
            decode_messages(&mut complete).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );

        let mut partial = vec![b'x'; MAX_ACP_FRAME_BYTES + 1];
        assert_eq!(
            decode_messages(&mut partial).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(
            finish_decode(&partial).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }
}
