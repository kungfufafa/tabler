//! The character encoding of a text export: the format writers produce UTF-8, which
//! [`Encoded`] passes through (UTF-8), prefixes with a byte order mark (UTF-8 with BOM) or
//! transcodes (UTF-16 LE with its BOM, Windows-1252, Latin-1) on the way to the file. Characters
//! the encoding cannot represent are written as `?` and counted.

use std::io::{self, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::import::text::single_byte_char;
use crate::types::export::{ExportEncoding, ExportFormat};
use crate::types::import::TextEncoding;

/// The encoding a format is written in: Excel workbooks are zip files (UTF-8 XML inside), never
/// transcoded.
pub fn effective_encoding(format: &ExportFormat, encoding: ExportEncoding) -> ExportEncoding {
    match format {
        ExportFormat::Xlsx { .. } => ExportEncoding::Utf8,
        _ => encoding,
    }
}

/// The `charset` an HTML page declares for `encoding`.
pub fn html_charset(encoding: ExportEncoding) -> &'static str {
    match encoding {
        ExportEncoding::Utf8 | ExportEncoding::Utf8Bom => "utf-8",
        ExportEncoding::Utf16Le => "utf-16",
        ExportEncoding::Windows1252 => "windows-1252",
        ExportEncoding::Latin1 => "iso-8859-1",
    }
}

/// The byte a single-byte encoding has for `c`, if any.
fn single_byte(encoding: ExportEncoding, c: char) -> Option<u8> {
    let code = c as u32;
    match encoding {
        ExportEncoding::Latin1 => u8::try_from(code).ok(),
        ExportEncoding::Windows1252 => {
            if code < 0x80 || (0xA0..=0xFF).contains(&code) {
                return Some(code as u8);
            }
            (0x80u8..=0x9F).find(|&b| single_byte_char(TextEncoding::Windows1252, b) == c)
        }
        _ => None,
    }
}

/// Writes UTF-8 text into `inner` in `encoding`.
pub struct Encoded<W: Write> {
    inner: W,
    encoding: ExportEncoding,
    /// The byte order mark is still to be written.
    bom_pending: bool,
    /// The start of a character cut by the end of the previous write.
    carry: Vec<u8>,
    /// Transcoded bytes of the current write (reused).
    buf: Vec<u8>,
    replaced: Arc<AtomicU64>,
}

impl<W: Write> Encoded<W> {
    /// `replaced` counts the characters written as `?`.
    pub fn new(inner: W, encoding: ExportEncoding, replaced: Arc<AtomicU64>) -> Encoded<W> {
        Encoded {
            inner,
            encoding,
            bom_pending: matches!(encoding, ExportEncoding::Utf8Bom | ExportEncoding::Utf16Le),
            carry: Vec::new(),
            buf: Vec::new(),
            replaced,
        }
    }

    fn write_bom(&mut self) -> io::Result<()> {
        if std::mem::take(&mut self.bom_pending) {
            let bom: &[u8] = match self.encoding {
                ExportEncoding::Utf16Le => &[0xFF, 0xFE],
                _ => &[0xEF, 0xBB, 0xBF],
            };
            self.inner.write_all(bom)?;
        }
        Ok(())
    }

    fn push(&mut self, c: char) {
        match self.encoding {
            ExportEncoding::Utf16Le => {
                let mut units = [0u16; 2];
                for unit in c.encode_utf16(&mut units) {
                    self.buf.extend_from_slice(&unit.to_le_bytes());
                }
            }
            ExportEncoding::Utf8 | ExportEncoding::Utf8Bom => {
                let mut tmp = [0u8; 4];
                self.buf
                    .extend_from_slice(c.encode_utf8(&mut tmp).as_bytes());
            }
            ExportEncoding::Windows1252 | ExportEncoding::Latin1 => {
                match single_byte(self.encoding, c) {
                    Some(b) => self.buf.push(b),
                    None => {
                        self.buf.push(b'?');
                        self.replaced.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        }
    }

    /// Transcodes `data` (UTF-8, possibly cut inside a character at its end) into `buf`; the cut
    /// character waits in `carry`. Bytes that are not UTF-8 at all (never produced by the
    /// writers) become `?`.
    fn transcode(&mut self, mut data: &[u8]) {
        loop {
            match std::str::from_utf8(data) {
                Ok(text) => {
                    for c in text.chars() {
                        self.push(c);
                    }
                    return;
                }
                Err(e) => {
                    let (valid, rest) = data.split_at(e.valid_up_to());
                    // `valid_up_to` ends at a character boundary.
                    for c in std::str::from_utf8(valid).unwrap_or_default().chars() {
                        self.push(c);
                    }
                    match e.error_len() {
                        None => {
                            self.carry.extend_from_slice(rest);
                            return;
                        }
                        Some(n) => {
                            self.buf.push(b'?');
                            self.replaced.fetch_add(1, Ordering::Relaxed);
                            data = &rest[n..];
                        }
                    }
                }
            }
        }
    }

    /// Writes what is pending (the byte order mark of an empty file, a cut character as `?`)
    /// and returns the inner writer.
    pub fn finish(mut self) -> io::Result<W> {
        self.write_bom()?;
        if !self.carry.is_empty() {
            self.carry.clear();
            self.buf.clear();
            self.buf.push(b'?');
            self.replaced.fetch_add(1, Ordering::Relaxed);
            if self.encoding == ExportEncoding::Utf16Le {
                self.buf.push(0);
            }
            self.inner.write_all(&self.buf)?;
        }
        self.inner.flush()?;
        Ok(self.inner)
    }
}

impl<W: Write> Write for Encoded<W> {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.write_bom()?;
        if matches!(
            self.encoding,
            ExportEncoding::Utf8 | ExportEncoding::Utf8Bom
        ) {
            return self.inner.write(data);
        }
        self.buf.clear();
        if self.carry.is_empty() {
            self.transcode(data);
        } else {
            let mut joined = std::mem::take(&mut self.carry);
            joined.extend_from_slice(data);
            self.transcode(&joined);
        }
        let buf = std::mem::take(&mut self.buf);
        let written = self.inner.write_all(&buf);
        self.buf = buf;
        written.map(|()| data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode(encoding: ExportEncoding, parts: &[&[u8]]) -> (Vec<u8>, u64) {
        let replaced = Arc::new(AtomicU64::new(0));
        let mut w = Encoded::new(Vec::new(), encoding, replaced.clone());
        for p in parts {
            w.write_all(p).unwrap();
        }
        (w.finish().unwrap(), replaced.load(Ordering::Relaxed))
    }

    #[test]
    fn utf8_passes_through_and_the_bom_comes_first() {
        let text = "id,name\n1,Zoë 😀\n".as_bytes();
        assert_eq!(encode(ExportEncoding::Utf8, &[text]), (text.to_vec(), 0));
        let (bom, n) = encode(ExportEncoding::Utf8Bom, &[&text[..5], &text[5..]]);
        assert_eq!(&bom[..3], &[0xEF, 0xBB, 0xBF]);
        assert_eq!(&bom[3..], text);
        assert_eq!(n, 0);
        // An empty file still gets its byte order mark.
        assert_eq!(
            encode(ExportEncoding::Utf8Bom, &[]).0,
            vec![0xEF, 0xBB, 0xBF]
        );
    }

    #[test]
    fn utf16_le_with_bom_and_surrogates() {
        let (out, n) = encode(ExportEncoding::Utf16Le, &["a€😀".as_bytes()]);
        assert_eq!(n, 0);
        let mut expected = vec![0xFF, 0xFE, b'a', 0, 0xAC, 0x20];
        for unit in "😀".encode_utf16() {
            expected.extend_from_slice(&unit.to_le_bytes());
        }
        assert_eq!(out, expected);
        // Decodes back.
        let units: Vec<u16> = out[2..]
            .chunks(2)
            .map(|p| u16::from_le_bytes([p[0], p[1]]))
            .collect();
        assert_eq!(String::from_utf16(&units).unwrap(), "a€😀");
    }

    #[test]
    fn single_byte_encodings_replace_what_they_cannot_hold() {
        let text = "Zoë € ‘q’ 世".as_bytes();
        let (cp, n) = encode(ExportEncoding::Windows1252, &[text]);
        assert_eq!(cp, b"Zo\xEB \x80 \x91q\x92 ?".to_vec());
        assert_eq!(n, 1);
        let (latin, n) = encode(ExportEncoding::Latin1, &[text]);
        assert_eq!(latin, b"Zo\xEB ? ?q? ?".to_vec());
        assert_eq!(n, 4);
    }

    #[test]
    fn characters_cut_between_writes_are_joined() {
        let text = "é€😀".as_bytes();
        // Every split point, including inside each character.
        for cut in 0..=text.len() {
            let (out, n) = encode(ExportEncoding::Utf16Le, &[&text[..cut], &text[cut..]]);
            assert_eq!(n, 0, "cut at {cut}");
            let units: Vec<u16> = out[2..]
                .chunks(2)
                .map(|p| u16::from_le_bytes([p[0], p[1]]))
                .collect();
            assert_eq!(String::from_utf16(&units).unwrap(), "é€😀");
        }
        // A character cut at the very end is a replacement.
        let (out, n) = encode(ExportEncoding::Windows1252, &[&"a€".as_bytes()[..2]]);
        assert_eq!(out, b"a?".to_vec());
        assert_eq!(n, 1);
    }

    #[test]
    fn excel_is_never_transcoded_and_html_names_its_charset() {
        assert_eq!(
            effective_encoding(
                &ExportFormat::Xlsx { header: true },
                ExportEncoding::Utf16Le
            ),
            ExportEncoding::Utf8
        );
        assert_eq!(
            effective_encoding(&ExportFormat::Html, ExportEncoding::Latin1),
            ExportEncoding::Latin1
        );
        assert_eq!(html_charset(ExportEncoding::Windows1252), "windows-1252");
        assert_eq!(html_charset(ExportEncoding::Utf8Bom), "utf-8");
    }
}
