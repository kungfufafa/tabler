//! Text decoding of imported files: UTF-8, UTF-16 LE / BE, Windows-1252 and Latin-1, streamed
//! into UTF-8 (a byte order mark is dropped). Detection from the first bytes: a byte order mark,
//! else UTF-16 when every other byte is NUL, else UTF-8 when the start is valid UTF-8, else
//! Windows-1252 (what TablePlus falls back to).

use std::io::{self, Read};

use crate::types::import::TextEncoding;

/// Windows-1252 at 0x80..=0x9F. Its five undefined bytes map to the C1 control of the same
/// value, as browsers decode them.
const CP1252_HIGH: [char; 32] = [
    '\u{20AC}', '\u{81}', '\u{201A}', '\u{192}', '\u{201E}', '\u{2026}', '\u{2020}', '\u{2021}',
    '\u{2C6}', '\u{2030}', '\u{160}', '\u{2039}', '\u{152}', '\u{8D}', '\u{17D}', '\u{8F}',
    '\u{90}', '\u{2018}', '\u{2019}', '\u{201C}', '\u{201D}', '\u{2022}', '\u{2013}', '\u{2014}',
    '\u{2DC}', '\u{2122}', '\u{161}', '\u{203A}', '\u{153}', '\u{9D}', '\u{17E}', '\u{178}',
];

/// A byte of a single-byte encoding as a character.
pub fn single_byte_char(encoding: TextEncoding, b: u8) -> char {
    match (encoding, b) {
        (TextEncoding::Windows1252, 0x80..=0x9F) => CP1252_HIGH[(b - 0x80) as usize],
        _ => b as char,
    }
}

/// Decodes `bytes` of a single-byte encoding.
pub fn decode_single_byte(encoding: TextEncoding, bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|&b| single_byte_char(encoding, b))
        .collect()
}

pub fn encoding_label(encoding: TextEncoding) -> &'static str {
    match encoding {
        TextEncoding::Utf8 => "UTF-8",
        TextEncoding::Utf16Le => "UTF-16 LE",
        TextEncoding::Utf16Be => "UTF-16 BE",
        TextEncoding::Windows1252 => "Windows-1252",
        TextEncoding::Latin1 => "Latin-1",
    }
}

/// Whether `bytes` is valid UTF-8, allowing a character cut at the end (a sample of a file).
pub fn is_utf8_prefix(bytes: &[u8]) -> bool {
    match std::str::from_utf8(bytes) {
        Ok(_) => true,
        // `error_len() == None`: the input ends inside a character.
        Err(e) => e.error_len().is_none(),
    }
}

/// The encoding of a file starting with `sample`.
pub fn detect_encoding(sample: &[u8]) -> TextEncoding {
    if sample.starts_with(&[0xEF, 0xBB, 0xBF]) {
        return TextEncoding::Utf8;
    }
    if sample.starts_with(&[0xFF, 0xFE]) {
        return TextEncoding::Utf16Le;
    }
    if sample.starts_with(&[0xFE, 0xFF]) {
        return TextEncoding::Utf16Be;
    }
    // ASCII text in UTF-16 without a byte order mark: NUL in every other byte.
    if sample.len() >= 4 {
        let pairs = sample.len() / 2;
        let zero_odd = sample
            .iter()
            .skip(1)
            .step_by(2)
            .filter(|&&b| b == 0)
            .count();
        let zero_even = sample.iter().step_by(2).filter(|&&b| b == 0).count();
        if zero_odd * 10 >= pairs * 9 && zero_even * 10 < pairs {
            return TextEncoding::Utf16Le;
        }
        if zero_even * 10 >= pairs * 9 && zero_odd * 10 < pairs {
            return TextEncoding::Utf16Be;
        }
    }
    if is_utf8_prefix(sample) {
        TextEncoding::Utf8
    } else {
        TextEncoding::Windows1252
    }
}

const READ_CHUNK: usize = 64 * 1024;

/// Reads `inner` as `encoding` and yields its text as UTF-8. UTF-8 input passes through
/// unchecked (the reader of the text reports invalid bytes with their position); UTF-16 that is
/// cut or holds a lone surrogate yields U+FFFD.
pub struct Utf8Reader<R: Read> {
    inner: R,
    encoding: TextEncoding,
    /// Decoded text not handed out yet, from `pos`.
    out: Vec<u8>,
    pos: usize,
    /// Undecoded bytes carried to the next read (half a UTF-16 unit, the start of a BOM).
    carry: Vec<u8>,
    /// UTF-16: a high surrogate waiting for its low half.
    high: Option<u16>,
    started: bool,
    eof: bool,
}

impl<R: Read> Utf8Reader<R> {
    pub fn new(inner: R, encoding: TextEncoding) -> Utf8Reader<R> {
        Utf8Reader {
            inner,
            encoding,
            out: Vec::new(),
            pos: 0,
            carry: Vec::new(),
            high: None,
            started: false,
            eof: false,
        }
    }

    pub fn encoding(&self) -> TextEncoding {
        self.encoding
    }

    fn push_char(&mut self, c: char) {
        let mut tmp = [0u8; 4];
        self.out
            .extend_from_slice(c.encode_utf8(&mut tmp).as_bytes());
    }

    fn fill(&mut self) -> io::Result<()> {
        self.out.clear();
        self.pos = 0;
        let mut raw = std::mem::take(&mut self.carry);
        let mut chunk = vec![0u8; READ_CHUNK];
        let n = loop {
            match self.inner.read(&mut chunk) {
                Ok(n) => break n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
        };
        if n == 0 {
            self.eof = true;
        }
        raw.extend_from_slice(&chunk[..n]);
        if !self.started {
            // A byte order mark is at most 3 bytes: wait for them.
            if raw.len() < 3 && !self.eof {
                self.carry = raw;
                return Ok(());
            }
            self.started = true;
            let bom: &[u8] = match self.encoding {
                TextEncoding::Utf8 => &[0xEF, 0xBB, 0xBF],
                TextEncoding::Utf16Le => &[0xFF, 0xFE],
                TextEncoding::Utf16Be => &[0xFE, 0xFF],
                TextEncoding::Windows1252 | TextEncoding::Latin1 => &[],
            };
            if !bom.is_empty() && raw.starts_with(bom) {
                raw.drain(..bom.len());
            }
        }
        match self.encoding {
            TextEncoding::Utf8 => self.out = raw,
            TextEncoding::Windows1252 | TextEncoding::Latin1 => {
                self.out.reserve(raw.len());
                for &b in &raw {
                    if b < 0x80 {
                        self.out.push(b);
                    } else {
                        let c = single_byte_char(self.encoding, b);
                        self.push_char(c);
                    }
                }
            }
            TextEncoding::Utf16Le | TextEncoding::Utf16Be => {
                let le = self.encoding == TextEncoding::Utf16Le;
                let whole = raw.len() / 2 * 2;
                for pair in raw[..whole].as_chunks::<2>().0 {
                    let unit = if le {
                        u16::from_le_bytes([pair[0], pair[1]])
                    } else {
                        u16::from_be_bytes([pair[0], pair[1]])
                    };
                    self.push_unit(unit);
                }
                if whole < raw.len() {
                    if self.eof {
                        self.push_char(char::REPLACEMENT_CHARACTER);
                    } else {
                        self.carry = raw[whole..].to_vec();
                    }
                }
                if self.eof && self.high.take().is_some() {
                    self.push_char(char::REPLACEMENT_CHARACTER);
                }
            }
        }
        Ok(())
    }

    fn push_unit(&mut self, unit: u16) {
        match (self.high.take(), unit) {
            (Some(high), 0xDC00..=0xDFFF) => {
                let c = 0x10000 + ((u32::from(high) - 0xD800) << 10) + (u32::from(unit) - 0xDC00);
                self.push_char(char::from_u32(c).unwrap_or(char::REPLACEMENT_CHARACTER));
            }
            (high, 0xD800..=0xDBFF) => {
                if high.is_some() {
                    self.push_char(char::REPLACEMENT_CHARACTER);
                }
                self.high = Some(unit);
            }
            (high, unit) => {
                if high.is_some() {
                    self.push_char(char::REPLACEMENT_CHARACTER);
                }
                let c = if (0xDC00..=0xDFFF).contains(&unit) {
                    char::REPLACEMENT_CHARACTER
                } else {
                    char::from_u32(u32::from(unit)).unwrap_or(char::REPLACEMENT_CHARACTER)
                };
                self.push_char(c);
            }
        }
    }
}

impl<R: Read> Read for Utf8Reader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        while self.pos >= self.out.len() {
            if self.eof {
                return Ok(0);
            }
            self.fill()?;
        }
        let n = buf.len().min(self.out.len() - self.pos);
        buf[..n].copy_from_slice(&self.out[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

/// Counts the bytes read through it (shared with the progress reporter).
pub struct CountingReader<R: Read> {
    inner: R,
    count: std::sync::Arc<std::sync::atomic::AtomicU64>,
}

impl<R: Read> CountingReader<R> {
    pub fn new(inner: R, count: std::sync::Arc<std::sync::atomic::AtomicU64>) -> Self {
        CountingReader { inner, count }
    }
}

impl<R: Read> Read for CountingReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.count
            .fetch_add(n as u64, std::sync::atomic::Ordering::Relaxed);
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Hands out at most `step` bytes per read, to cut units and characters anywhere.
    struct Trickle<'a> {
        data: &'a [u8],
        step: usize,
    }

    impl Read for Trickle<'_> {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let n = self.step.min(buf.len()).min(self.data.len());
            buf[..n].copy_from_slice(&self.data[..n]);
            self.data = &self.data[n..];
            Ok(n)
        }
    }

    fn decode(bytes: &[u8], encoding: TextEncoding, step: usize) -> String {
        let mut out = String::new();
        Utf8Reader::new(Trickle { data: bytes, step }, encoding)
            .read_to_string(&mut out)
            .unwrap();
        out
    }

    fn utf16(text: &str, le: bool, bom: bool) -> Vec<u8> {
        let mut out = Vec::new();
        if bom {
            out.extend_from_slice(if le { &[0xFF, 0xFE] } else { &[0xFE, 0xFF] });
        }
        for unit in text.encode_utf16() {
            out.extend_from_slice(&if le {
                unit.to_le_bytes()
            } else {
                unit.to_be_bytes()
            });
        }
        out
    }

    #[test]
    fn decodes_every_encoding_in_any_chunking() {
        let text = "id,name\n1,Zoë 😀 – “quoted”\n";
        for step in [1, 2, 3, 5, 64 * 1024] {
            let mut utf8 = vec![0xEF, 0xBB, 0xBF];
            utf8.extend_from_slice(text.as_bytes());
            assert_eq!(decode(&utf8, TextEncoding::Utf8, step), text, "step {step}");
            assert_eq!(decode(text.as_bytes(), TextEncoding::Utf8, step), text);
            for (le, enc) in [
                (true, TextEncoding::Utf16Le),
                (false, TextEncoding::Utf16Be),
            ] {
                assert_eq!(decode(&utf16(text, le, true), enc, step), text);
                assert_eq!(decode(&utf16(text, le, false), enc, step), text);
            }
        }
        // Windows-1252 maps 0x80–0x9F to punctuation; Latin-1 to C1 controls.
        let bytes = [b'a', 0xE9, 0x80, 0x93, 0x94, 0xFF];
        assert_eq!(decode(&bytes, TextEncoding::Windows1252, 2), "aé€“”ÿ");
        assert_eq!(
            decode(&bytes, TextEncoding::Latin1, 2),
            "aé\u{80}\u{93}\u{94}ÿ"
        );
        assert_eq!(decode(b"", TextEncoding::Utf8, 1), "");
        assert_eq!(decode(b"ab", TextEncoding::Utf8, 1), "ab");
    }

    #[test]
    fn broken_utf16_becomes_replacement_characters() {
        // A lone high surrogate, a lone low surrogate, an odd trailing byte.
        let bytes = [0x00, 0xD8, 0x41, 0x00, 0x00, 0xDC, 0x42];
        assert_eq!(
            decode(&bytes, TextEncoding::Utf16Le, 1),
            "\u{FFFD}A\u{FFFD}\u{FFFD}"
        );
    }

    #[test]
    fn detects_encodings() {
        assert_eq!(detect_encoding(b"\xEF\xBB\xBFa,b"), TextEncoding::Utf8);
        assert_eq!(detect_encoding(b"\xFF\xFEa\0"), TextEncoding::Utf16Le);
        assert_eq!(detect_encoding(b"\xFE\xFF\0a"), TextEncoding::Utf16Be);
        assert_eq!(
            detect_encoding(&utf16("id,name\n1,x", true, false)),
            TextEncoding::Utf16Le
        );
        assert_eq!(
            detect_encoding(&utf16("id,name\n1,x", false, false)),
            TextEncoding::Utf16Be
        );
        assert_eq!(detect_encoding("Zoë".as_bytes()), TextEncoding::Utf8);
        // A sample cut inside a character is still UTF-8.
        assert_eq!(detect_encoding(&"Zoë".as_bytes()[..3]), TextEncoding::Utf8);
        assert_eq!(detect_encoding(b"Zo\xEB,x"), TextEncoding::Windows1252);
        assert_eq!(detect_encoding(b""), TextEncoding::Utf8);
    }
}
