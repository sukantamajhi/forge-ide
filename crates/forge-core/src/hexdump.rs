//! A classic hex dump (offset, 16 bytes in hex, printable text) for files that are not text.

/// Dump at most `limit` bytes of `bytes`. A note at the end says how much was left out.
pub fn hex_dump(bytes: &[u8], limit: usize) -> String {
    let shown = &bytes[..bytes.len().min(limit)];
    let mut out = String::with_capacity(shown.len() * 4 + 64);
    for (row, chunk) in shown.chunks(16).enumerate() {
        out.push_str(&format!("{:08x}  ", row * 16));
        for i in 0..16 {
            match chunk.get(i) {
                Some(b) => out.push_str(&format!("{b:02x} ")),
                None => out.push_str("   "),
            }
            if i == 7 {
                out.push(' ');
            }
        }
        out.push(' ');
        out.extend(chunk.iter().map(|&b| if (0x20..0x7f).contains(&b) { b as char } else { '.' }));
        out.push('\n');
    }
    if bytes.len() > shown.len() {
        out.push_str(&format!("... {} more bytes not shown (the first {} of {} are listed)\n", bytes.len() - shown.len(), shown.len(), bytes.len()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_show_offset_hex_and_text() {
        let dump = hex_dump(b"Hello, Forge!\x00\xff and more text", 1000);
        let first = dump.lines().next().unwrap();
        assert!(first.starts_with("00000000  48 65 6c 6c 6f 2c 20 46  6f 72 67 65 21 00 ff 20 "), "{first}");
        assert!(first.ends_with("Hello, Forge!.. "), "{first}");
        assert!(dump.lines().nth(1).unwrap().starts_with("00000010  "));
    }

    #[test]
    fn long_input_is_cut_with_a_note() {
        let dump = hex_dump(&[7u8; 100], 32);
        assert_eq!(dump.lines().count(), 3);
        assert!(dump.contains("68 more bytes not shown"), "{dump}");
        assert_eq!(hex_dump(&[], 10), "");
    }
}
