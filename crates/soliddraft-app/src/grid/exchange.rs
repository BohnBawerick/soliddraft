//! The configuration table as CSV and XLSX. Both formats decode to the same logical
//! cells, and one header grammar maps them to the model's table by name. Nothing here
//! evaluates: expressions go to the model's value graph, and a spreadsheet formula is
//! refused even when it carries a usable cached result.

use quick_xml::{Reader, events::Event};
use soliddraft_model::configs::{DEFAULT_ROW, Grid, GridRow, GridValue};
use std::collections::BTreeSet;
use std::io::{Cursor, Read, Write};
use zip::{ZipArchive, ZipWriter, write::SimpleFileOptions};

/// The data sheet's name in a workbook.
const SHEET: &str = "Configurations";
/// The first header: the column of row names.
const NAMES: &str = "configuration";
/// The largest table file this reads, and the most any one workbook part may unpack to.
pub(crate) const MAX_BYTES: u64 = 16 * 1024 * 1024;
/// Rows and columns a table can hold: the header and row names, and the model's 256 rows
/// and 256 values with as many properties.
const MAX_ROWS: usize = 258;
const MAX_COLUMNS: usize = 1024;

const MAIN: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";
const RELATIONSHIPS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const PACKAGE: &str = "http://schemas.openxmlformats.org/package/2006/relationships";

/// A table file's format.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Csv,
    Xlsx,
}

impl Format {
    pub fn parse(text: &str) -> Result<Format, String> {
        match text {
            "csv" => Ok(Format::Csv),
            "xlsx" => Ok(Format::Xlsx),
            other => Err(format!("the format is csv or xlsx, not `{other}`")),
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Format::Csv => "csv",
            Format::Xlsx => "xlsx",
        }
    }
}

/// Reads a file's cells: the header, then each row, all the header's width.
pub fn decode(format: Format, bytes: &[u8]) -> Result<Vec<Vec<String>>, String> {
    let table = match format {
        Format::Csv => read_csv(bytes)?,
        Format::Xlsx => read_xlsx(bytes)?,
    };
    check_shape(&table)?;
    Ok(table)
}

/// Writes logical cells as a file.
pub fn encode(format: Format, table: &[Vec<String>]) -> Result<Vec<u8>, String> {
    match format {
        Format::Csv => Ok(write_csv(table)),
        Format::Xlsx => write_xlsx(table),
    }
}

fn check_shape(table: &[Vec<String>]) -> Result<(), String> {
    let width = table.first().map_or(0, Vec::len);
    if width == 0 {
        return Err("the table has no header".into());
    }
    if table.len() > MAX_ROWS || width > MAX_COLUMNS {
        return Err(format!("a table holds at most {MAX_ROWS} rows and {MAX_COLUMNS} columns"));
    }
    Ok(())
}

/// The header cell of a named-value column.
fn value_header(value: &GridValue) -> String {
    format!("value:{} [{}]", value.name, value.unit)
}

/// The header cell of a text property column.
fn property_header(name: &str) -> String {
    format!("property:{name} [text]")
}

/// The table's cells: its header, then one row per configuration. Blanks stay blank.
pub fn cells(grid: &Grid) -> Vec<Vec<String>> {
    let mut header = vec![NAMES.to_owned()];
    header.extend(grid.values.iter().map(value_header));
    header.extend(grid.properties.iter().map(|p| property_header(p)));
    let mut table = vec![header];
    table.extend(grid.rows.iter().map(|row| {
        let mut out = vec![row.name.clone()];
        out.extend(grid.values.iter().map(|v| row.values.get(&v.name).cloned().unwrap_or_default()));
        out.extend(grid.properties.iter().map(|p| row.properties.get(p).cloned().unwrap_or_default()));
        out
    }));
    table
}

enum Column {
    Name,
    Value(GridValue),
    Property(String),
}

fn column(index: usize, text: &str) -> Result<Column, String> {
    if text == NAMES {
        return Ok(Column::Name);
    }
    let bad = || format!("column {}: `{text}` is not value:name [unit] or property:name [text]", index + 1);
    let (kind, rest) = text.split_once(':').ok_or_else(bad)?;
    let (name, unit) = rest.strip_suffix(']').and_then(|r| r.rsplit_once(" [")).ok_or_else(bad)?;
    match (kind, name.is_empty()) {
        ("value", false) => Ok(Column::Value(GridValue { name: name.into(), unit: unit.into() })),
        ("property", false) if unit == "text" => Ok(Column::Property(name.into())),
        ("property", false) => Err(format!("column {}: property {name} has type {unit}, not text", index + 1)),
        _ => Err(bad()),
    }
}

/// The name a header gives its column, for a message about a cell under it.
fn column_name(text: &str) -> &str {
    text.split_once(':').map_or(text, |(_, rest)| rest.rsplit_once(" [").map_or(rest, |(name, _)| name))
}

/// Maps logical cells to a candidate table by header name, whatever the column order.
pub fn grid_of(table: &[Vec<String>]) -> Result<Grid, String> {
    check_shape(table)?;
    let columns = table[0].iter().enumerate().map(|(i, text)| column(i, text)).collect::<Result<Vec<_>, _>>()?;
    let mut names = None;
    let mut grid = Grid::default();
    let mut seen = BTreeSet::new();
    for (i, column) in columns.iter().enumerate() {
        let (unique, text) = match column {
            Column::Name => (names.replace(i).is_none(), NAMES),
            Column::Value(v) => (seen.insert(("value", v.name.as_str())), v.name.as_str()),
            Column::Property(p) => (seen.insert(("property", p.as_str())), p.as_str()),
        };
        if !unique {
            return Err(format!("duplicate column {text}"));
        }
        match column {
            Column::Name => {}
            Column::Value(v) => grid.values.push(v.clone()),
            Column::Property(p) => grid.properties.push(p.clone()),
        }
    }
    let names = names.ok_or_else(|| format!("missing column {NAMES}"))?;
    for (number, cells) in table.iter().enumerate().skip(1) {
        if cells.len() != columns.len() {
            return Err(format!("row {} has {} cells, the header has {}", number + 1, cells.len(), columns.len()));
        }
        let name = cells[names].as_str();
        if name.is_empty() {
            return Err(format!("row {}: missing configuration name", number + 1));
        }
        let mut row = GridRow { name: name.to_owned(), ..GridRow::default() };
        for (cell, column) in cells.iter().zip(&columns) {
            match column {
                Column::Value(v) if !cell.trim().is_empty() => {
                    row.values.insert(v.name.clone(), cell.trim().to_owned());
                }
                Column::Property(p) if !cell.is_empty() => {
                    row.properties.insert(p.clone(), cell.clone());
                }
                _ => {}
            }
        }
        grid.rows.push(row);
    }
    if !grid.rows.iter().any(|r| r.name == DEFAULT_ROW) {
        return Err(format!("missing configuration {DEFAULT_ROW}"));
    }
    Ok(grid)
}

// ----- CSV -----

fn read_csv(bytes: &[u8]) -> Result<Vec<Vec<String>>, String> {
    let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    let text = std::str::from_utf8(bytes).map_err(|_| "CSV is not UTF-8".to_owned())?;
    let mut records: Vec<(usize, Vec<String>)> = Vec::new();
    let (mut number, mut record, mut field, mut quoted) = (1, Vec::new(), String::new(), false);
    let mut chars = text.chars().peekable();
    let mut end_record = |number: &mut usize, record: &mut Vec<String>, field: &mut String, quoted: &mut bool| {
        record.push(std::mem::take(field));
        // An empty line is no record; a row of commas is one.
        if record.len() > 1 || !record[0].is_empty() || *quoted {
            records.push((*number, std::mem::take(record)));
        }
        record.clear();
        *quoted = false;
        *number += 1;
    };
    while let Some(c) = chars.next() {
        match c {
            '"' if field.is_empty() && !quoted => {
                quoted = true;
                loop {
                    match chars.next() {
                        None => return Err(format!("CSV row {number}: a quoted field never closes")),
                        Some('"') if chars.peek() == Some(&'"') => {
                            chars.next();
                            field.push('"');
                        }
                        Some('"') => break,
                        Some(inner) => field.push(inner),
                    }
                }
                if !matches!(chars.peek(), None | Some(',' | '\r' | '\n')) {
                    return Err(format!("CSV row {number}: text follows a closing quote"));
                }
            }
            '"' => return Err(format!("CSV row {number}: a quote inside an unquoted field")),
            ',' => {
                record.push(std::mem::take(&mut field));
                quoted = false;
            }
            '\r' if chars.peek() == Some(&'\n') => {}
            '\n' => end_record(&mut number, &mut record, &mut field, &mut quoted),
            other => field.push(other),
        }
    }
    if !field.is_empty() || !record.is_empty() || quoted {
        end_record(&mut number, &mut record, &mut field, &mut quoted);
    }
    let width = records.first().map_or(0, |(_, r)| r.len());
    if let Some((number, record)) = records.iter().find(|(_, r)| r.len() != width) {
        return Err(format!("CSV row {number} has {} fields, the header has {width}", record.len()));
    }
    Ok(records.into_iter().map(|(_, r)| r).collect())
}

fn write_csv(table: &[Vec<String>]) -> Vec<u8> {
    let field = |text: &String| {
        let quote = text.contains([',', '"', '\r', '\n']) || text.trim() != text;
        if quote { format!("\"{}\"", text.replace('"', "\"\"")) } else { text.clone() }
    };
    let mut out = String::new();
    for row in table {
        out.push_str(&row.iter().map(field).collect::<Vec<_>>().join(","));
        out.push_str("\r\n");
    }
    out.into_bytes()
}

// ----- XLSX -----

fn unreadable(e: impl std::fmt::Display) -> String {
    format!("XLSX workbook cannot be read: {e}")
}

/// A part of the package, unpacked up to the size limit.
fn part(zip: &mut ZipArchive<Cursor<&[u8]>>, name: &str) -> Result<Option<String>, String> {
    let file = match zip.by_name(name) {
        Ok(file) => file,
        Err(zip::result::ZipError::FileNotFound) => return Ok(None),
        Err(e) => return Err(unreadable(e)),
    };
    let mut text = String::new();
    file.take(MAX_BYTES + 1).read_to_string(&mut text).map_err(unreadable)?;
    if text.len() as u64 > MAX_BYTES {
        return Err(unreadable(format!("{name} is too large")));
    }
    Ok(Some(text))
}

/// An element's attribute by local name.
fn attribute(e: &quick_xml::events::BytesStart<'_>, name: &[u8]) -> Result<Option<String>, String> {
    for a in e.attributes() {
        let a = a.map_err(unreadable)?;
        if a.key.local_name().as_ref() == name {
            let value = a.normalized_value(quick_xml::XmlVersion::Implicit1_0).map_err(unreadable)?;
            return Ok(Some(value.into_owned()));
        }
    }
    Ok(None)
}

fn reference(e: &quick_xml::events::BytesRef<'_>) -> Result<Option<char>, String> {
    if let Some(c) = e.resolve_char_ref().map_err(unreadable)? {
        return Ok(Some(c));
    }
    Ok(match e.decode().map_err(unreadable)?.as_ref() {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        _ => None,
    })
}

/// The worksheet part holding the data sheet, as a path in the package.
fn sheet_path(zip: &mut ZipArchive<Cursor<&[u8]>>) -> Result<String, String> {
    let workbook = part(zip, "xl/workbook.xml")?.ok_or_else(|| unreadable("no workbook part"))?;
    let mut reader = Reader::from_str(&workbook);
    reader.config_mut().expand_empty_elements = true;
    let mut id = None;
    loop {
        match reader.read_event().map_err(unreadable)? {
            Event::Start(e) if e.local_name().as_ref() == b"sheet" => {
                if attribute(&e, b"name")?.as_deref() == Some(SHEET) {
                    id = attribute(&e, b"id")?;
                    break;
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    let id = id.ok_or_else(|| format!("XLSX has no sheet named {SHEET}"))?;
    let rels = part(zip, "xl/_rels/workbook.xml.rels")?.ok_or_else(|| unreadable("no workbook relationships"))?;
    let mut reader = Reader::from_str(&rels);
    reader.config_mut().expand_empty_elements = true;
    loop {
        match reader.read_event().map_err(unreadable)? {
            Event::Start(e) if e.local_name().as_ref() == b"Relationship" => {
                if attribute(&e, b"Id")?.as_deref() == Some(id.as_str()) {
                    let target = attribute(&e, b"Target")?.ok_or_else(|| unreadable("a relationship has no target"))?;
                    return Ok(match target.strip_prefix('/') {
                        Some(path) => path.to_owned(),
                        None => format!("xl/{target}"),
                    });
                }
            }
            Event::Eof => return Err(unreadable(format!("sheet {SHEET} has no part"))),
            _ => {}
        }
    }
}

fn shared_strings(zip: &mut ZipArchive<Cursor<&[u8]>>) -> Result<Vec<String>, String> {
    let Some(xml) = part(zip, "xl/sharedStrings.xml")? else { return Ok(Vec::new()) };
    let mut reader = Reader::from_str(&xml);
    reader.config_mut().expand_empty_elements = true;
    let (mut strings, mut current, mut in_text, mut phonetic) = (Vec::new(), String::new(), false, false);
    loop {
        match reader.read_event().map_err(unreadable)? {
            Event::Start(e) => match e.local_name().as_ref() {
                b"t" => in_text = !phonetic,
                b"rPh" => phonetic = true,
                _ => {}
            },
            Event::End(e) => match e.local_name().as_ref() {
                b"t" => in_text = false,
                b"rPh" => phonetic = false,
                b"si" => strings.push(std::mem::take(&mut current)),
                _ => {}
            },
            Event::Text(t) if in_text => current.push_str(&t.xml10_content().map_err(unreadable)?),
            Event::GeneralRef(e) if in_text => current.extend(reference(&e)?),
            Event::CData(t) if in_text => current.push_str(&t.decode().map_err(unreadable)?),
            Event::Eof => return Ok(strings),
            _ => {}
        }
    }
}

/// "B3" as zero-based column and row.
fn cell_at(reference: &str) -> Option<(usize, usize)> {
    let digits = reference.find(|c: char| c.is_ascii_digit())?;
    let (letters, number) = reference.split_at(digits);
    if letters.is_empty() || !letters.bytes().all(|b| b.is_ascii_uppercase()) {
        return None;
    }
    let column = letters.bytes().try_fold(0usize, |n, b| n.checked_mul(26)?.checked_add((b - b'A' + 1) as usize))?;
    Some((column - 1, number.parse::<usize>().ok()?.checked_sub(1)?))
}

fn column_letters(mut column: usize) -> String {
    let mut out = Vec::new();
    loop {
        out.push(b'A' + (column % 26) as u8);
        if column < 26 {
            break;
        }
        column = column / 26 - 1;
    }
    out.reverse();
    String::from_utf8(out).expect("ASCII")
}

/// One worksheet cell as it is being read.
#[derive(Default)]
struct Cell {
    at: Option<(usize, usize)>,
    kind: Option<String>,
    formula: bool,
    value: Option<String>,
    text: String,
}

/// What the sheet holds: the value cells by place, and where formulas are.
#[derive(Default)]
struct SheetCells {
    cells: Vec<(usize, usize, String)>,
    formulas: Vec<(usize, usize)>,
}

fn finish_cell(cell: Cell, shared: &[String], out: &mut SheetCells) -> Result<(), String> {
    let (column, row) = cell.at.ok_or_else(|| unreadable("a cell has no position"))?;
    let name = format!("{SHEET}!{}{}", column_letters(column), row + 1);
    if cell.formula {
        out.formulas.push((column, row));
        return Ok(());
    }
    let text = match cell.kind.as_deref() {
        Some("s") => {
            let index = cell.value.as_deref().unwrap_or_default().trim().parse::<usize>();
            let found = index.ok().and_then(|i| shared.get(i));
            found.cloned().ok_or_else(|| unreadable(format!("{name} names no shared string")))?
        }
        Some("inlineStr") => cell.text,
        Some("str") | Some("n") | None => cell.value.unwrap_or_default(),
        Some(other) => return Err(format!("{name}: XLSX cell type {other} cannot be imported")),
    };
    if !text.is_empty() {
        out.cells.push((column, row, text));
    }
    Ok(())
}

fn read_sheet(xml: &str, shared: &[String]) -> Result<SheetCells, String> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().expand_empty_elements = true;
    let mut out = SheetCells::default();
    let (mut row, mut column) = (0usize, 0usize);
    let mut cell: Option<Cell> = None;
    let (mut in_value, mut in_text) = (false, false);
    loop {
        match reader.read_event().map_err(unreadable)? {
            Event::Start(e) => match e.local_name().as_ref() {
                b"row" => {
                    column = 0;
                    row = match attribute(&e, b"r")? {
                        Some(r) => {
                            let n = r.parse::<usize>().map_err(|_| unreadable(format!("bad row {r}")))?;
                            n.checked_sub(1).ok_or_else(|| unreadable("row 0"))?
                        }
                        None => row + 1,
                    };
                }
                b"c" => {
                    let at = match attribute(&e, b"r")? {
                        Some(r) => cell_at(&r).ok_or_else(|| unreadable(format!("bad cell reference {r}")))?,
                        None => (column, row),
                    };
                    column = at.0 + 1;
                    cell = Some(Cell { at: Some(at), kind: attribute(&e, b"t")?, ..Cell::default() });
                }
                b"f" => {
                    if let Some(c) = &mut cell {
                        c.formula = true;
                    }
                }
                b"v" => in_value = cell.is_some(),
                b"t" => in_text = cell.is_some(),
                _ => {}
            },
            Event::End(e) => match e.local_name().as_ref() {
                b"c" => {
                    if let Some(done) = cell.take() {
                        finish_cell(done, shared, &mut out)?;
                    }
                }
                b"v" => in_value = false,
                b"t" => in_text = false,
                _ => {}
            },
            Event::Text(t) if in_value || in_text => {
                let text = t.xml10_content().map_err(unreadable)?;
                push_text(&mut cell, in_value, &text);
            }
            Event::CData(t) if in_value || in_text => {
                let text = t.decode().map_err(unreadable)?;
                push_text(&mut cell, in_value, &text);
            }
            Event::GeneralRef(e) if in_value || in_text => {
                if let Some(c) = reference(&e)? {
                    push_text(&mut cell, in_value, c.encode_utf8(&mut [0; 4]));
                }
            }
            Event::Eof => return Ok(out),
            _ => {}
        }
    }
}

fn push_text(cell: &mut Option<Cell>, value: bool, text: &str) {
    if let Some(cell) = cell {
        if value {
            cell.value.get_or_insert_default().push_str(text);
        } else {
            cell.text.push_str(text);
        }
    }
}

fn read_xlsx(bytes: &[u8]) -> Result<Vec<Vec<String>>, String> {
    let mut zip = ZipArchive::new(Cursor::new(bytes)).map_err(unreadable)?;
    let path = sheet_path(&mut zip)?;
    let shared = shared_strings(&mut zip)?;
    let xml = part(&mut zip, &path)?.ok_or_else(|| unreadable(format!("no worksheet part {path}")))?;
    let sheet = read_sheet(&xml, &shared)?;
    let used = sheet.cells.iter().map(|&(c, r, _)| (c, r)).chain(sheet.formulas.iter().copied());
    let (mut width, mut height) = (0, 0);
    for (column, row) in used {
        width = width.max(column + 1);
        height = height.max(row + 1);
    }
    if height > 1024 || width > MAX_COLUMNS {
        return Err(format!("sheet {SHEET} is larger than a configuration table can be"));
    }
    let mut table = vec![vec![String::new(); width]; height];
    for (column, row, text) in sheet.cells {
        table[row][column] = text;
    }
    if let Some(&(column, row)) = sheet.formulas.first() {
        let place = format!("{SHEET}!{}{}", column_letters(column), row + 1);
        let header = column_name(table.first().and_then(|h| h.get(column)).map_or("", String::as_str));
        let name = table[row].first().map_or("", String::as_str);
        return Err(format!("{place} ({name}, {header}): an XLSX formula cannot be imported"));
    }
    let width = table.first().map_or(0, |h| h.iter().rposition(|c| !c.is_empty()).map_or(0, |i| i + 1));
    for (number, row) in table.iter_mut().enumerate() {
        if let Some(extra) = row.iter().skip(width).position(|c| !c.is_empty()) {
            return Err(format!("{SHEET}!{}{}: a cell outside the header", column_letters(width + extra), number + 1));
        }
        row.truncate(width);
    }
    table.retain(|row| row.iter().any(|c| !c.is_empty()));
    Ok(table)
}

fn xml_text(text: &str) -> Result<String, String> {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\t' | '\n' | '\r' => out.push_str(&format!("&#{};", c as u32)),
            c if c.is_control() => return Err("a table cell holds a control character XLSX cannot store".into()),
            c => out.push(c),
        }
    }
    Ok(out)
}

fn write_xlsx(table: &[Vec<String>]) -> Result<Vec<u8>, String> {
    let mut sheet = format!(r#"<?xml version="1.0" encoding="UTF-8"?><worksheet xmlns="{MAIN}"><sheetData>"#);
    for (r, row) in table.iter().enumerate() {
        sheet.push_str(&format!(r#"<row r="{}">"#, r + 1));
        for (c, text) in row.iter().enumerate().filter(|(_, text)| !text.is_empty()) {
            sheet.push_str(&format!(
                r#"<c r="{}{}" t="inlineStr"><is><t xml:space="preserve">{}</t></is></c>"#,
                column_letters(c),
                r + 1,
                xml_text(text)?
            ));
        }
        sheet.push_str("</row>");
    }
    sheet.push_str("</sheetData></worksheet>");
    let parts = [
        (
            "[Content_Types].xml",
            r#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/></Types>"#.to_owned(),
        ),
        (
            "_rels/.rels",
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="{PACKAGE}"><Relationship Id="rId1" Type="{RELATIONSHIPS}/officeDocument" Target="xl/workbook.xml"/></Relationships>"#
            ),
        ),
        (
            "xl/workbook.xml",
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?><workbook xmlns="{MAIN}" xmlns:r="{RELATIONSHIPS}"><sheets><sheet name="{SHEET}" sheetId="1" r:id="rId1"/></sheets></workbook>"#
            ),
        ),
        (
            "xl/_rels/workbook.xml.rels",
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="{PACKAGE}"><Relationship Id="rId1" Type="{RELATIONSHIPS}/worksheet" Target="worksheets/sheet1.xml"/></Relationships>"#
            ),
        ),
        ("xl/worksheets/sheet1.xml", sheet),
    ];
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let failed = |e: &dyn std::fmt::Display| format!("the workbook could not be written: {e}");
    for (name, text) in parts {
        zip.start_file(name, SimpleFileOptions::default()).map_err(|e| failed(&e))?;
        zip.write_all(text.as_bytes()).map_err(|e| failed(&e))?;
    }
    Ok(zip.finish().map_err(|e| failed(&e))?.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(rows: &[&[&str]]) -> Vec<Vec<String>> {
        rows.iter().map(|r| r.iter().map(|c| (*c).to_owned()).collect()).collect()
    }

    fn sample() -> Vec<Vec<String>> {
        table(&[
            &["configuration", "value:plate_w [mm]", "property:title [text]"],
            &["Default", "140", "Bracket Ø12, \"wide\""],
            &["Wide, \"shop\"", "", " spaced "],
        ])
    }

    #[test]
    fn csv_and_xlsx_return_the_same_cells() {
        for format in [Format::Csv, Format::Xlsx] {
            let bytes = encode(format, &sample()).unwrap();
            assert_eq!(decode(format, &bytes).unwrap(), sample(), "{format:?}");
        }
    }

    #[test]
    fn csv_takes_lf_and_a_byte_order_mark_and_refuses_broken_quoting() {
        let cells = decode(Format::Csv, b"\xef\xbb\xbfa,b\n1,\"2\n3\"\n").unwrap();
        assert_eq!(cells, table(&[&["a", "b"], &["1", "2\n3"]]));
        for (bytes, reason) in [
            (&b"a,b\n\"1,2\n"[..], "never closes"),
            (&b"a,b\n\"1\"x,2\n"[..], "follows a closing quote"),
            (&b"a,b\n1\"2,3\n"[..], "inside an unquoted field"),
            (&b"a,b\n1\n"[..], "CSV row 2 has 1 fields"),
            (&b"a,b\n\xff,1\n"[..], "not UTF-8"),
            (&b""[..], "no header"),
        ] {
            let error = decode(Format::Csv, bytes).unwrap_err();
            assert!(error.contains(reason), "{error}");
        }
    }

    #[test]
    fn malformed_headers_and_layouts_are_named() {
        for (rows, reason) in [
            (table(&[&["configuration", "plate_w"]]), "not value:name [unit]"),
            (table(&[&["configuration", ""]]), "not value:name [unit]"),
            (table(&[&["configuration", "value: [mm]"]]), "not value:name [unit]"),
            (table(&[&["configuration", "property:title [number]"]]), "not text"),
            (table(&[&["value:a [mm]"]]), "missing column configuration"),
            (table(&[&["configuration", "configuration"]]), "duplicate column configuration"),
            (table(&[&["configuration", "value:a [mm]", "value:a [mm]"]]), "duplicate column a"),
            (table(&[&["configuration"], &["Narrow"]]), "missing configuration Default"),
            (table(&[&["configuration", "value:a [mm]"], &["", "1"]]), "row 2: missing configuration name"),
            (table(&[&["configuration", "value:a [mm]"], &["Default"]]), "row 2 has 1 cells"),
        ] {
            let error = grid_of(&rows).unwrap_err();
            assert!(error.contains(reason), "{error}");
        }
    }

    #[test]
    fn a_workbook_without_the_data_sheet_or_with_a_stray_cell_is_refused() {
        let bytes = encode(Format::Xlsx, &sample()).unwrap();
        let mut zip = ZipArchive::new(Cursor::new(&bytes[..])).unwrap();
        let mut out = ZipWriter::new(Cursor::new(Vec::new()));
        for i in 0..zip.len() {
            let mut file = zip.by_index(i).unwrap();
            let name = file.name().unwrap().to_string();
            let mut text = String::new();
            file.read_to_string(&mut text).unwrap();
            out.start_file(name, SimpleFileOptions::default()).unwrap();
            out.write_all(text.replace("name=\"Configurations\"", "name=\"Sheet1\"").as_bytes()).unwrap();
        }
        let renamed = out.finish().unwrap().into_inner();
        assert!(decode(Format::Xlsx, &renamed).unwrap_err().contains("no sheet named Configurations"));
        let stray =
            "<sheetData xmlns=\"x\"><row r=\"1\"><c r=\"A1\" t=\"inlineStr\"><is><t>a</t></is></c></row></sheetData>";
        assert_eq!(read_sheet(stray, &[]).unwrap().cells, [(0, 0, "a".to_owned())]);
        assert_eq!(cell_at("AA10"), Some((26, 9)));
        assert_eq!(column_letters(26), "AA");
    }
}
