//! LSP-style header framing (PROTOCOL.md §1.1): `Content-Length: N\r\n\r\n<body>`.

use std::io::{self, BufRead, Write};

/// Reads one framed message body. Returns `None` on clean EOF at a message
/// boundary.
pub fn read_message(reader: &mut impl BufRead) -> io::Result<Option<Vec<u8>>> {
    let mut content_length: Option<usize> = None;
    let mut line = String::new();
    loop {
        line.clear();
        let n = reader.read_line(&mut line)?;
        if n == 0 {
            return if content_length.is_none() {
                Ok(None) // EOF between messages
            } else {
                Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "EOF inside message header",
                ))
            };
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break; // end of headers
        }
        if let Some(value) = trimmed
            .split_once(':')
            .filter(|(name, _)| name.eq_ignore_ascii_case("Content-Length"))
            .map(|(_, v)| v.trim())
        {
            content_length = Some(value.parse().map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidData, "bad Content-Length")
            })?);
        }
        // Content-Type is accepted and ignored (utf-8 is mandatory anyway).
    }
    let len = content_length.ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidData, "missing Content-Length header")
    })?;
    let mut body = vec![0u8; len];
    reader.read_exact(&mut body)?;
    Ok(Some(body))
}

pub fn write_message(writer: &mut impl Write, body: &[u8]) -> io::Result<()> {
    write!(writer, "Content-Length: {}\r\n\r\n", body.len())?;
    writer.write_all(body)?;
    writer.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn round_trip() {
        let mut buf = Vec::new();
        write_message(&mut buf, br#"{"jsonrpc":"2.0"}"#).unwrap();
        write_message(&mut buf, b"{}").unwrap();
        let mut cur = Cursor::new(buf);
        assert_eq!(
            read_message(&mut cur).unwrap().unwrap(),
            br#"{"jsonrpc":"2.0"}"#
        );
        assert_eq!(read_message(&mut cur).unwrap().unwrap(), b"{}");
        assert!(read_message(&mut cur).unwrap().is_none());
    }

    #[test]
    fn header_case_and_content_type() {
        let raw = b"content-length: 2\r\nContent-Type: application/vscode-jsonrpc; charset=utf-8\r\n\r\n{}";
        let mut cur = Cursor::new(raw.to_vec());
        assert_eq!(read_message(&mut cur).unwrap().unwrap(), b"{}");
    }

    #[test]
    fn missing_length_is_error() {
        let mut cur = Cursor::new(b"Content-Type: foo\r\n\r\n{}".to_vec());
        assert!(read_message(&mut cur).is_err());
    }
}
