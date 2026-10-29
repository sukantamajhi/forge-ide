use std::io::{self, BufRead};

use serde_json::Value;

/// Frame a JSON-RPC message with the LSP `Content-Length` header.
pub fn encode_message(message: &Value) -> Vec<u8> {
    let body = serde_json::to_vec(message).expect("Value always serializes");
    let mut out = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    out.extend_from_slice(&body);
    out
}

/// Read one framed message. `Ok(None)` means a clean EOF between messages.
pub fn read_message<R: BufRead>(reader: &mut R) -> io::Result<Option<Value>> {
    let mut content_length: Option<usize> = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            return if content_length.is_none() {
                Ok(None)
            } else {
                Err(io::ErrorKind::UnexpectedEof.into())
            };
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            if name.trim().eq_ignore_ascii_case("content-length") {
                content_length = Some(value.trim().parse().map_err(|_| {
                    io::Error::new(io::ErrorKind::InvalidData, "invalid Content-Length")
                })?);
            }
        }
    }
    let len = content_length
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing Content-Length"))?;
    let mut body = vec![0u8; len];
    reader.read_exact(&mut body)?;
    serde_json::from_slice(&body)
        .map(Some)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn roundtrip_two_messages() {
        let mut bytes = encode_message(&json!({"id": 1, "method": "a"}));
        bytes.extend(encode_message(&json!({"id": 2, "method": "é"})));
        let mut reader = io::Cursor::new(bytes);
        assert_eq!(read_message(&mut reader).unwrap().unwrap()["id"], 1);
        // Content-Length counts bytes, so the multibyte payload must still parse.
        assert_eq!(read_message(&mut reader).unwrap().unwrap()["method"], "é");
        assert!(read_message(&mut reader).unwrap().is_none());
    }

    #[test]
    fn ignores_extra_headers_and_header_case() {
        let raw = b"content-length: 2\r\nContent-Type: application/vscode-jsonrpc; charset=utf-8\r\n\r\n{}";
        let mut reader = io::Cursor::new(&raw[..]);
        assert_eq!(read_message(&mut reader).unwrap().unwrap(), json!({}));
    }

    #[test]
    fn rejects_missing_length_and_truncated_body() {
        let mut reader = io::Cursor::new(&b"X-Foo: 1\r\n\r\n{}"[..]);
        assert!(read_message(&mut reader).is_err());

        let mut reader = io::Cursor::new(&b"Content-Length: 10\r\n\r\n{}"[..]);
        assert!(read_message(&mut reader).is_err());
    }
}
