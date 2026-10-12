//! A minimal streaming Excel (`.xlsx`) writer: a zip archive written front to back (every entry
//! deflated and followed by a data descriptor, so nothing seeks), with one worksheet per table.
//!
//! Text cells are inline strings (`t="inlineStr"`), so nothing is kept per value: a shared string
//! table would have to hold every distinct string until the end. Numbers that a spreadsheet can
//! hold exactly (integers, finite floats, decimals of at most 15 significant digits) are number
//! cells; big integers and longer decimals stay text so no digit is lost. Booleans are boolean
//! cells; dates and times are their database text. The header row is bold and frozen.
//!
//! Limits (Excel's): 1,048,576 rows and 16,384 columns per sheet, 32,767 characters per cell
//! (longer text is cut), 31 characters per sheet name. No Zip64: a workbook over 4 GB fails.

use std::io::{self, Write};
use std::sync::LazyLock;

use flate2::write::DeflateEncoder;
use flate2::{Compression, Crc};
use regex::Regex;

use crate::error::AppError;
use crate::types::value::Value;

use super::writer::float_text;

/// Rows per sheet, header included.
pub const MAX_ROWS: u64 = 1_048_576;
pub const MAX_COLUMNS: usize = 16_384;
/// Characters per cell; longer text is cut to this.
pub const MAX_CELL_CHARS: usize = 32_767;
const MAX_SHEET_NAME: usize = 31;
/// Compressed bytes buffered before they go to the output.
const DRAIN_AT: usize = 64 * 1024;

const SIG_LOCAL: u32 = 0x0403_4b50;
const SIG_DESCRIPTOR: u32 = 0x0807_4b50;
const SIG_CENTRAL: u32 = 0x0201_4b50;
const SIG_END: u32 = 0x0605_4b50;
/// Bit 3: sizes and CRC follow the data; bit 11: UTF-8 names.
const FLAGS: u16 = 0x0808;
const DEFLATE: u16 = 8;
const VERSION: u16 = 20;

fn too_big() -> io::Error {
    io::Error::other("the Excel file would be larger than 4 GB, which .xlsx files cannot be")
}

fn u32_of(n: u64) -> io::Result<u32> {
    u32::try_from(n).map_err(|_| too_big())
}

struct CentralEntry {
    name: String,
    crc: u32,
    compressed: u32,
    size: u32,
    offset: u32,
}

struct OpenEntry {
    name: String,
    offset: u64,
    enc: DeflateEncoder<Vec<u8>>,
    crc: Crc,
    size: u64,
    compressed: u64,
}

/// A zip archive written sequentially into `w`.
pub struct ZipStream<W: Write> {
    w: W,
    pos: u64,
    entries: Vec<CentralEntry>,
    open: Option<OpenEntry>,
    time: u16,
    date: u16,
}

impl<W: Write> ZipStream<W> {
    pub fn new(w: W) -> ZipStream<W> {
        let (time, date) = dos_now();
        ZipStream {
            w,
            pos: 0,
            entries: Vec::new(),
            open: None,
            time,
            date,
        }
    }

    fn put(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.w.write_all(bytes)?;
        self.pos += bytes.len() as u64;
        Ok(())
    }

    /// Starts the entry `name` (the previous one must be ended).
    pub fn start(&mut self, name: &str) -> io::Result<()> {
        if self.open.is_some() {
            return Err(io::Error::other("a zip entry is still open"));
        }
        let offset = self.pos;
        let mut head = Vec::with_capacity(30 + name.len());
        head.extend_from_slice(&SIG_LOCAL.to_le_bytes());
        head.extend_from_slice(&VERSION.to_le_bytes());
        head.extend_from_slice(&FLAGS.to_le_bytes());
        head.extend_from_slice(&DEFLATE.to_le_bytes());
        head.extend_from_slice(&self.time.to_le_bytes());
        head.extend_from_slice(&self.date.to_le_bytes());
        head.extend_from_slice(&[0; 12]); // CRC and sizes: in the data descriptor
        head.extend_from_slice(&(name.len() as u16).to_le_bytes());
        head.extend_from_slice(&0u16.to_le_bytes());
        head.extend_from_slice(name.as_bytes());
        self.put(&head)?;
        self.open = Some(OpenEntry {
            name: name.to_string(),
            offset,
            enc: DeflateEncoder::new(Vec::new(), Compression::fast()),
            crc: Crc::new(),
            size: 0,
            compressed: 0,
        });
        Ok(())
    }

    /// Appends to the open entry.
    pub fn write(&mut self, data: &[u8]) -> io::Result<()> {
        let entry = self
            .open
            .as_mut()
            .ok_or_else(|| io::Error::other("no zip entry is open"))?;
        entry.crc.update(data);
        entry.size += data.len() as u64;
        entry.enc.write_all(data)?;
        if entry.enc.get_ref().len() >= DRAIN_AT {
            // The encoder only ever appends to its output: taking it out is safe.
            let chunk = std::mem::take(entry.enc.get_mut());
            entry.compressed += chunk.len() as u64;
            self.put(&chunk)?;
        }
        Ok(())
    }

    /// Ends the open entry: the rest of its data, then its data descriptor.
    pub fn end(&mut self) -> io::Result<()> {
        let entry = self
            .open
            .take()
            .ok_or_else(|| io::Error::other("no zip entry is open"))?;
        let rest = entry.enc.finish()?;
        let compressed = entry.compressed + rest.len() as u64;
        self.put(&rest)?;
        let crc = entry.crc.sum();
        let (compressed, size, offset) = (
            u32_of(compressed)?,
            u32_of(entry.size)?,
            u32_of(entry.offset)?,
        );
        let mut desc = Vec::with_capacity(16);
        desc.extend_from_slice(&SIG_DESCRIPTOR.to_le_bytes());
        desc.extend_from_slice(&crc.to_le_bytes());
        desc.extend_from_slice(&compressed.to_le_bytes());
        desc.extend_from_slice(&size.to_le_bytes());
        self.put(&desc)?;
        self.entries.push(CentralEntry {
            name: entry.name,
            crc,
            compressed,
            size,
            offset,
        });
        Ok(())
    }

    /// A whole entry at once.
    pub fn file(&mut self, name: &str, data: &[u8]) -> io::Result<()> {
        self.start(name)?;
        self.write(data)?;
        self.end()
    }

    /// Writes the central directory and returns the output, flushed.
    pub fn finish(mut self) -> io::Result<W> {
        if self.open.is_some() {
            self.end()?;
        }
        let cd_start = self.pos;
        let entries = std::mem::take(&mut self.entries);
        for e in &entries {
            let mut rec = Vec::with_capacity(46 + e.name.len());
            rec.extend_from_slice(&SIG_CENTRAL.to_le_bytes());
            rec.extend_from_slice(&VERSION.to_le_bytes()); // made by
            rec.extend_from_slice(&VERSION.to_le_bytes()); // needed
            rec.extend_from_slice(&FLAGS.to_le_bytes());
            rec.extend_from_slice(&DEFLATE.to_le_bytes());
            rec.extend_from_slice(&self.time.to_le_bytes());
            rec.extend_from_slice(&self.date.to_le_bytes());
            rec.extend_from_slice(&e.crc.to_le_bytes());
            rec.extend_from_slice(&e.compressed.to_le_bytes());
            rec.extend_from_slice(&e.size.to_le_bytes());
            rec.extend_from_slice(&(e.name.len() as u16).to_le_bytes());
            rec.extend_from_slice(&[0; 8]); // extra, comment, disk, internal attributes
            rec.extend_from_slice(&0u32.to_le_bytes()); // external attributes
            rec.extend_from_slice(&e.offset.to_le_bytes());
            rec.extend_from_slice(e.name.as_bytes());
            self.put(&rec)?;
        }
        let cd_size = u32_of(self.pos - cd_start)?;
        let cd_start = u32_of(cd_start)?;
        let count = u16::try_from(entries.len()).map_err(|_| too_big())?;
        let mut end = Vec::with_capacity(22);
        end.extend_from_slice(&SIG_END.to_le_bytes());
        end.extend_from_slice(&[0; 4]); // disk numbers
        end.extend_from_slice(&count.to_le_bytes());
        end.extend_from_slice(&count.to_le_bytes());
        end.extend_from_slice(&cd_size.to_le_bytes());
        end.extend_from_slice(&cd_start.to_le_bytes());
        end.extend_from_slice(&0u16.to_le_bytes());
        self.put(&end)?;
        self.w.flush()?;
        Ok(self.w)
    }
}

/// The local time as MS-DOS time and date (what zip entries carry).
fn dos_now() -> (u16, u16) {
    use chrono::{Datelike, Timelike};
    let now = chrono::Local::now();
    let year = now.year().clamp(1980, 2107) as u16;
    let time =
        ((now.hour() as u16) << 11) | ((now.minute() as u16) << 5) | (now.second() as u16 / 2);
    let date = ((year - 1980) << 9) | ((now.month() as u16) << 5) | now.day() as u16;
    (time, date)
}

// ---------------------------------------------------------------------------------------------
// XML text
// ---------------------------------------------------------------------------------------------

/// `_xHHHH_` as written literally in the data: its underscore must be escaped, or Excel would
/// read it as an escaped character.
static ESCAPE_LIKE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^_x[0-9A-Fa-f]{4}_").expect("valid escape regex"));

/// Escapes text for an XML element or attribute. Characters XML 1.0 cannot hold (control
/// characters other than tab and line breaks, U+FFFE, U+FFFF) become Excel's `_xHHHH_` escapes.
pub fn xml_text(s: &str, out: &mut String) {
    for (i, c) in s.char_indices() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '_' if ESCAPE_LIKE.is_match(&s[i..]) => out.push_str("_x005F_"),
            '\t' | '\n' | '\r' => out.push(c),
            c if (c as u32) < 0x20 || c == '\u{FFFE}' || c == '\u{FFFF}' => {
                out.push_str(&format!("_x{:04X}_", c as u32));
            }
            c => out.push(c),
        }
    }
}

/// `A`, `B`, … `Z`, `AA`, … for a 0-based column index.
pub fn column_letters(mut index: usize) -> String {
    let mut out = Vec::new();
    loop {
        out.push(b'A' + (index % 26) as u8);
        if index < 26 {
            break;
        }
        index = index / 26 - 1;
    }
    out.reverse();
    String::from_utf8(out).unwrap_or_default()
}

static NUMERIC: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[+-]?(\d+(\.\d*)?|\.\d+)([eE][+-]?\d+)?$").expect("valid numeric regex")
});

/// Whether a decimal's text is a number a spreadsheet (a double) holds without losing a digit.
fn exact_in_double(s: &str) -> bool {
    if !NUMERIC.is_match(s) {
        return false;
    }
    let mantissa = s.split(['e', 'E']).next().unwrap_or(s);
    let digits = mantissa.trim_start_matches(['+', '-']).replace('.', "");
    let significant = digits.trim_start_matches('0').trim_end_matches('0').len();
    significant <= 15
}

/// A sheet name Excel accepts, unique (case-insensitively) among `taken`.
pub fn sheet_name(wanted: &str, taken: &[String]) -> String {
    let cleaned: String = wanted
        .chars()
        .map(|c| match c {
            '[' | ']' | ':' | '*' | '?' | '/' | '\\' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let cleaned = cleaned.trim().trim_matches('\'').trim();
    let mut base: String = cleaned.chars().take(MAX_SHEET_NAME).collect();
    if base.is_empty() {
        base = "Sheet".into();
    }
    // A sheet of this name is reserved by Excel.
    if base.eq_ignore_ascii_case("history") {
        base.push('_');
    }
    let clash = |name: &str| {
        taken
            .iter()
            .any(|t| t.to_lowercase() == name.to_lowercase())
    };
    if !clash(&base) {
        return base;
    }
    for n in 2.. {
        let suffix = format!(" ({n})");
        let keep = MAX_SHEET_NAME.saturating_sub(suffix.chars().count());
        let candidate: String = base.chars().take(keep).collect::<String>() + &suffix;
        if !clash(&candidate) {
            return candidate;
        }
    }
    unreachable!("an unused sheet name always exists")
}

// ---------------------------------------------------------------------------------------------
// Workbook
// ---------------------------------------------------------------------------------------------

const MAIN_NS: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";
const REL_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const PKG_REL_NS: &str = "http://schemas.openxmlformats.org/package/2006/relationships";
const XML_HEAD: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n";

const STYLES: &str = concat!(
    "<styleSheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\">",
    "<fonts count=\"2\"><font><sz val=\"11\"/><name val=\"Calibri\"/></font>",
    "<font><b/><sz val=\"11\"/><name val=\"Calibri\"/></font></fonts>",
    "<fills count=\"2\"><fill><patternFill patternType=\"none\"/></fill>",
    "<fill><patternFill patternType=\"gray125\"/></fill></fills>",
    "<borders count=\"1\"><border><left/><right/><top/><bottom/><diagonal/></border></borders>",
    "<cellStyleXfs count=\"1\"><xf numFmtId=\"0\" fontId=\"0\" fillId=\"0\" borderId=\"0\"/></cellStyleXfs>",
    "<cellXfs count=\"2\"><xf numFmtId=\"0\" fontId=\"0\" fillId=\"0\" borderId=\"0\" xfId=\"0\"/>",
    "<xf numFmtId=\"0\" fontId=\"1\" fillId=\"0\" borderId=\"0\" xfId=\"0\" applyFont=\"1\"/></cellXfs>",
    "<cellStyles count=\"1\"><cellStyle name=\"Normal\" xfId=\"0\" builtinId=\"0\"/></cellStyles>",
    "</styleSheet>"
);

struct OpenSheet {
    /// Rows written, header included.
    rows: u64,
}

/// Writes worksheets into a workbook. `begin` starts a sheet (named by `next_sheet`, or
/// `Sheet<n>`), `write_row` adds rows, `end_sheet` closes it; `finish` writes the workbook.
pub struct XlsxWriter<W: Write> {
    zip: ZipStream<W>,
    header: bool,
    sheets: Vec<String>,
    next_name: Option<String>,
    open: Option<OpenSheet>,
    line: String,
}

fn io_err(e: io::Error) -> AppError {
    AppError::Io(e.to_string())
}

impl<W: Write> XlsxWriter<W> {
    pub fn new(out: W, header: bool) -> XlsxWriter<W> {
        XlsxWriter {
            zip: ZipStream::new(out),
            header,
            sheets: Vec::new(),
            next_name: None,
            open: None,
            line: String::new(),
        }
    }

    /// Names the sheet the next `begin` starts.
    pub fn next_sheet(&mut self, name: &str) {
        self.next_name = Some(name.to_string());
    }

    /// Starts a sheet with these columns (ending the open one).
    pub fn begin(&mut self, columns: &[String]) -> Result<(), AppError> {
        if self.open.is_some() {
            self.end_sheet()?;
        }
        if columns.len() > MAX_COLUMNS {
            return Err(AppError::Unsupported(format!(
                "an Excel sheet holds at most {MAX_COLUMNS} columns, this result has {}",
                columns.len()
            )));
        }
        let wanted = self
            .next_name
            .take()
            .unwrap_or_else(|| format!("Sheet{}", self.sheets.len() + 1));
        let name = sheet_name(&wanted, &self.sheets);
        self.sheets.push(name);
        let n = self.sheets.len();
        self.zip
            .start(&format!("xl/worksheets/sheet{n}.xml"))
            .map_err(io_err)?;
        let mut head = format!("{XML_HEAD}<worksheet xmlns=\"{MAIN_NS}\" xmlns:r=\"{REL_NS}\">");
        if self.header {
            head.push_str(
                "<sheetViews><sheetView workbookViewId=\"0\"><pane ySplit=\"1\" topLeftCell=\"A2\" \
                 activePane=\"bottomLeft\" state=\"frozen\"/></sheetView></sheetViews>",
            );
        }
        head.push_str("<sheetData>");
        self.zip.write(head.as_bytes()).map_err(io_err)?;
        self.open = Some(OpenSheet { rows: 0 });
        if self.header {
            let names: Vec<Value> = columns.iter().map(|c| Value::Text(c.clone())).collect();
            self.push_row(&names, true)?;
        }
        Ok(())
    }

    pub fn write_row(&mut self, row: &[Value]) -> Result<(), AppError> {
        self.push_row(row, false)
    }

    fn push_row(&mut self, row: &[Value], bold: bool) -> Result<(), AppError> {
        let sheet = self
            .open
            .as_mut()
            .ok_or_else(|| AppError::internal("no Excel sheet is open"))?;
        if sheet.rows >= MAX_ROWS {
            return Err(AppError::Unsupported(
                "an Excel sheet holds at most 1,048,576 rows; export as CSV instead".into(),
            ));
        }
        sheet.rows += 1;
        let r = sheet.rows;
        let line = &mut self.line;
        line.clear();
        line.push_str(&format!("<row r=\"{r}\">"));
        for (i, v) in row.iter().enumerate().take(MAX_COLUMNS) {
            let at = format!("{}{r}", column_letters(i));
            let style = if bold { " s=\"1\"" } else { "" };
            match v {
                Value::Null | Value::Default => {}
                Value::Bool(b) => {
                    line.push_str(&format!(
                        "<c r=\"{at}\"{style} t=\"b\"><v>{}</v></c>",
                        u8::from(*b)
                    ));
                }
                Value::Int(n) => line.push_str(&format!("<c r=\"{at}\"{style}><v>{n}</v></c>")),
                Value::Float(f) if f.is_finite() => line.push_str(&format!(
                    "<c r=\"{at}\"{style}><v>{}</v></c>",
                    float_text(*f)
                )),
                Value::Decimal(s) if exact_in_double(s) => {
                    line.push_str(&format!("<c r=\"{at}\"{style}><v>{s}</v></c>"));
                }
                other => {
                    let text = match other {
                        Value::Float(f) => f.to_string(),
                        _ => other.as_text().unwrap_or_default(),
                    };
                    line.push_str(&format!(
                        "<c r=\"{at}\"{style} t=\"inlineStr\"><is><t xml:space=\"preserve\">"
                    ));
                    if text.chars().count() > MAX_CELL_CHARS {
                        let cut: String = text.chars().take(MAX_CELL_CHARS).collect();
                        xml_text(&cut, line);
                    } else {
                        xml_text(&text, line);
                    }
                    line.push_str("</t></is></c>");
                }
            }
        }
        line.push_str("</row>");
        self.zip.write(self.line.as_bytes()).map_err(io_err)
    }

    /// Closes the open sheet (nothing when none is open).
    pub fn end_sheet(&mut self) -> Result<(), AppError> {
        if self.open.take().is_none() {
            return Ok(());
        }
        self.zip
            .write(b"</sheetData></worksheet>")
            .map_err(io_err)?;
        self.zip.end().map_err(io_err)
    }

    /// Ends the open sheet and writes the workbook's other parts. A workbook without any sheet
    /// gets an empty one (Excel refuses a workbook without sheets).
    pub fn finish(mut self) -> Result<W, AppError> {
        self.end_sheet()?;
        if self.sheets.is_empty() {
            self.begin(&[])?;
            self.end_sheet()?;
        }
        let n = self.sheets.len();

        let mut workbook =
            format!("{XML_HEAD}<workbook xmlns=\"{MAIN_NS}\" xmlns:r=\"{REL_NS}\"><sheets>");
        for (i, name) in self.sheets.iter().enumerate() {
            workbook.push_str("<sheet name=\"");
            xml_text(name, &mut workbook);
            workbook.push_str(&format!("\" sheetId=\"{}\" r:id=\"rId{}\"/>", i + 1, i + 1));
        }
        workbook.push_str("</sheets></workbook>");

        let mut rels = format!("{XML_HEAD}<Relationships xmlns=\"{PKG_REL_NS}\">");
        for i in 1..=n {
            rels.push_str(&format!(
                "<Relationship Id=\"rId{i}\" Type=\"{REL_NS}/worksheet\" Target=\"worksheets/sheet{i}.xml\"/>"
            ));
        }
        rels.push_str(&format!(
            "<Relationship Id=\"rId{}\" Type=\"{REL_NS}/styles\" Target=\"styles.xml\"/></Relationships>",
            n + 1
        ));

        let mut types = format!(
            "{XML_HEAD}<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
             <Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\
             <Default Extension=\"xml\" ContentType=\"application/xml\"/>\
             <Override PartName=\"/xl/workbook.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml\"/>\
             <Override PartName=\"/xl/styles.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml\"/>"
        );
        for i in 1..=n {
            types.push_str(&format!(
                "<Override PartName=\"/xl/worksheets/sheet{i}.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml\"/>"
            ));
        }
        types.push_str("</Types>");

        let root_rels = format!(
            "{XML_HEAD}<Relationships xmlns=\"{PKG_REL_NS}\"><Relationship Id=\"rId1\" \
             Type=\"{REL_NS}/officeDocument\" Target=\"xl/workbook.xml\"/></Relationships>"
        );

        let styles = format!("{XML_HEAD}{STYLES}");
        for (name, data) in [
            ("xl/workbook.xml", workbook.as_str()),
            ("xl/_rels/workbook.xml.rels", rels.as_str()),
            ("xl/styles.xml", styles.as_str()),
            ("_rels/.rels", root_rels.as_str()),
            ("[Content_Types].xml", types.as_str()),
        ] {
            self.zip.file(name, data.as_bytes()).map_err(io_err)?;
        }
        self.zip.finish().map_err(io_err)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::io::Read;

    /// Reads a zip archive the way a strict reader does: through the central directory, checking
    /// every entry's local header, its data descriptor and its CRC. Returns name → content.
    pub fn unzip(bytes: &[u8]) -> BTreeMap<String, Vec<u8>> {
        let u16_at = |i: usize| u16::from_le_bytes([bytes[i], bytes[i + 1]]) as usize;
        let u32_at = |i: usize| {
            u32::from_le_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]) as usize
        };
        let end = bytes.len() - 22;
        assert_eq!(u32_at(end) as u32, SIG_END, "end of central directory");
        let count = u16_at(end + 10);
        let cd_size = u32_at(end + 12);
        let mut at = u32_at(end + 16);
        assert_eq!(
            at + cd_size,
            end,
            "central directory ends where the end record starts"
        );
        let mut out = BTreeMap::new();
        for _ in 0..count {
            assert_eq!(u32_at(at) as u32, SIG_CENTRAL);
            let crc = u32_at(at + 16) as u32;
            let csize = u32_at(at + 20);
            let size = u32_at(at + 24);
            let name_len = u16_at(at + 28);
            let offset = u32_at(at + 42);
            let name = String::from_utf8(bytes[at + 46..at + 46 + name_len].to_vec()).unwrap();
            at += 46 + name_len;

            assert_eq!(u32_at(offset) as u32, SIG_LOCAL, "local header of {name}");
            assert_eq!(u16_at(offset + 8) as u16, DEFLATE);
            let local_name_len = u16_at(offset + 26);
            let extra = u16_at(offset + 28);
            let data = offset + 30 + local_name_len + extra;
            let mut content = Vec::new();
            flate2::read::DeflateDecoder::new(&bytes[data..data + csize])
                .read_to_end(&mut content)
                .unwrap();
            assert_eq!(content.len(), size, "size of {name}");
            let mut check = Crc::new();
            check.update(&content);
            assert_eq!(check.sum(), crc, "CRC of {name}");
            let desc = data + csize;
            assert_eq!(u32_at(desc) as u32, SIG_DESCRIPTOR);
            assert_eq!(u32_at(desc + 4) as u32, crc);
            out.insert(name, content);
        }
        out
    }

    fn text(files: &BTreeMap<String, Vec<u8>>, name: &str) -> String {
        String::from_utf8(files[name].clone()).unwrap()
    }

    #[test]
    fn zip_round_trips_through_a_strict_reader() {
        let mut zip = ZipStream::new(Vec::new());
        zip.file("a.txt", b"hello").unwrap();
        zip.start("dir/big.txt").unwrap();
        let big: Vec<u8> = (0..300_000u32).flat_map(|i| i.to_le_bytes()).collect();
        for chunk in big.chunks(7919) {
            zip.write(chunk).unwrap();
        }
        zip.end().unwrap();
        zip.file("empty", b"").unwrap();
        let bytes = zip.finish().unwrap();
        let files = unzip(&bytes);
        assert_eq!(files["a.txt"], b"hello");
        assert_eq!(files["dir/big.txt"], big);
        assert!(files["empty"].is_empty());
        assert!(zip_misuse_is_an_error());
    }

    fn zip_misuse_is_an_error() -> bool {
        let mut zip = ZipStream::new(Vec::new());
        zip.write(b"x").is_err() && zip.end().is_err() && {
            zip.start("a").unwrap();
            zip.start("b").is_err()
        }
    }

    type Sheet<'a> = (&'a str, &'a [&'a str], Vec<Vec<Value>>);

    fn workbook(sheets: &[Sheet<'_>], header: bool) -> Vec<u8> {
        let mut w = XlsxWriter::new(Vec::new(), header);
        for (name, cols, rows) in sheets {
            w.next_sheet(name);
            let cols: Vec<String> = cols.iter().map(|c| c.to_string()).collect();
            w.begin(&cols).unwrap();
            for row in rows {
                w.write_row(row).unwrap();
            }
            w.end_sheet().unwrap();
        }
        w.finish().unwrap()
    }

    #[test]
    fn writes_numbers_as_numbers_and_text_inline() {
        let rows = vec![
            vec![
                Value::Int(42),
                Value::Text("a & <b> \"c\"".into()),
                Value::Float(1.5),
                Value::Bool(true),
                Value::Null,
                Value::Decimal("12.50".into()),
                Value::BigInt("9007199254740993".into()),
                Value::Decimal("0.1234567890123456789".into()),
                Value::Date("2026-01-02".into()),
            ],
            vec![
                Value::Int(-1),
                Value::Text("tab\there\u{1}_x0041_".into()),
                Value::Float(f64::NAN),
                Value::Bool(false),
                Value::Text(String::new()),
                Value::Decimal("NaN".into()),
                Value::Null,
                Value::Null,
                Value::Null,
            ],
        ];
        let bytes = workbook(
            &[(
                "users",
                &["id", "name", "f", "ok", "n", "dec", "big", "long", "d"],
                rows,
            )],
            true,
        );
        let files = unzip(&bytes);
        for part in [
            "[Content_Types].xml",
            "_rels/.rels",
            "xl/workbook.xml",
            "xl/_rels/workbook.xml.rels",
            "xl/styles.xml",
            "xl/worksheets/sheet1.xml",
        ] {
            assert!(files.contains_key(part), "{part} missing");
        }
        let sheet = text(&files, "xl/worksheets/sheet1.xml");
        assert!(sheet.contains("state=\"frozen\""));
        // Header: bold inline strings.
        assert!(sheet.contains(
            "<row r=\"1\"><c r=\"A1\" s=\"1\" t=\"inlineStr\"><is><t xml:space=\"preserve\">id</t></is></c>"
        ));
        // Row 2: numbers, escaped text, boolean, an empty cell for NULL.
        assert!(sheet.contains("<c r=\"A2\"><v>42</v></c>"), "{sheet}");
        assert!(sheet.contains(
            "<c r=\"B2\" t=\"inlineStr\"><is><t xml:space=\"preserve\">a &amp; &lt;b&gt; &quot;c&quot;</t></is></c>"
        ));
        assert!(sheet.contains("<c r=\"C2\"><v>1.5</v></c>"));
        assert!(sheet.contains("<c r=\"D2\" t=\"b\"><v>1</v></c>"));
        assert!(!sheet.contains("r=\"E2\""));
        assert!(sheet.contains("<c r=\"F2\"><v>12.50</v></c>"));
        // Values a double cannot hold exactly stay text.
        assert!(sheet.contains(
            "<c r=\"G2\" t=\"inlineStr\"><is><t xml:space=\"preserve\">9007199254740993</t>"
        ));
        assert!(sheet.contains("<c r=\"H2\" t=\"inlineStr\">"));
        assert!(
            sheet.contains(
                "<c r=\"I2\" t=\"inlineStr\"><is><t xml:space=\"preserve\">2026-01-02</t>"
            )
        );
        // Row 3: control characters and literal escapes, NaN as text, '' as an empty string.
        assert!(
            sheet.contains("tab\there_x0001__x005F_x0041_</t>"),
            "{sheet}"
        );
        assert!(
            sheet.contains("<c r=\"C3\" t=\"inlineStr\"><is><t xml:space=\"preserve\">NaN</t>")
        );
        assert!(sheet.contains("<c r=\"D3\" t=\"b\"><v>0</v></c>"));
        assert!(
            sheet.contains(
                "<c r=\"E3\" t=\"inlineStr\"><is><t xml:space=\"preserve\"></t></is></c>"
            )
        );
        let wb = text(&files, "xl/workbook.xml");
        assert!(wb.contains("<sheet name=\"users\" sheetId=\"1\" r:id=\"rId1\"/>"));
        let types = text(&files, "[Content_Types].xml");
        assert!(types.contains("/xl/worksheets/sheet1.xml"));
    }

    #[test]
    fn one_sheet_per_table_with_unique_names() {
        let row = || vec![vec![Value::Int(1)]];
        let bytes = workbook(
            &[
                ("orders", &["id"], row()),
                ("Orders", &["id"], row()),
                ("a/b:c", &["id"], row()),
                ("x".repeat(40).as_str(), &["id"], row()),
            ],
            false,
        );
        let files = unzip(&bytes);
        let wb = text(&files, "xl/workbook.xml");
        assert!(wb.contains("name=\"orders\""));
        assert!(wb.contains("name=\"Orders (2)\""));
        assert!(wb.contains("name=\"a_b_c\""));
        assert!(wb.contains(&format!("name=\"{}\"", "x".repeat(31))));
        for i in 1..=4 {
            assert!(files.contains_key(&format!("xl/worksheets/sheet{i}.xml")));
        }
        let rels = text(&files, "xl/_rels/workbook.xml.rels");
        assert!(rels.contains("Target=\"worksheets/sheet4.xml\""));
        assert!(rels.contains("Id=\"rId5\""));
        // No header row: no frozen pane, data starts at row 1.
        let sheet = text(&files, "xl/worksheets/sheet1.xml");
        assert!(!sheet.contains("frozen"));
        assert!(sheet.contains("<row r=\"1\"><c r=\"A1\"><v>1</v></c></row>"));
    }

    #[test]
    fn empty_workbook_still_has_a_sheet() {
        let bytes = XlsxWriter::new(Vec::new(), true).finish().unwrap();
        let files = unzip(&bytes);
        assert!(text(&files, "xl/workbook.xml").contains("name=\"Sheet1\""));
    }

    #[test]
    fn helpers() {
        assert_eq!(column_letters(0), "A");
        assert_eq!(column_letters(25), "Z");
        assert_eq!(column_letters(26), "AA");
        assert_eq!(column_letters(701), "ZZ");
        assert_eq!(column_letters(702), "AAA");
        assert_eq!(column_letters(MAX_COLUMNS - 1), "XFD");
        assert!(exact_in_double("123456789012345"));
        assert!(exact_in_double("-0.000123"));
        assert!(exact_in_double("1.5e10"));
        assert!(!exact_in_double("1234567890123456"));
        assert!(!exact_in_double("Infinity"));
        assert_eq!(sheet_name("History", &[]), "History_");
        assert_eq!(sheet_name("'quoted'", &[]), "quoted");
        assert_eq!(sheet_name("", &[]), "Sheet");
        assert_eq!(sheet_name("t", &["T".into(), "t (2)".into()]), "t (3)");
        let long = "y".repeat(31);
        assert_eq!(
            sheet_name(&long, std::slice::from_ref(&long)),
            format!("{} (2)", "y".repeat(27))
        );
    }

    #[test]
    fn long_text_is_cut_to_the_cell_limit() {
        let bytes = workbook(
            &[(
                "t",
                &["v"],
                vec![vec![Value::Text("é".repeat(MAX_CELL_CHARS + 10))]],
            )],
            false,
        );
        let files = unzip(&bytes);
        let sheet = text(&files, "xl/worksheets/sheet1.xml");
        assert_eq!(sheet.matches('é').count(), MAX_CELL_CHARS);
    }
}
