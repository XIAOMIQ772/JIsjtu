//! read_office：读取 PowerPoint（.pptx）、Word（.docx）、Excel（.xlsx）里的文字。
//!
//! 这三种文件都是 ZIP 包里的 XML。这里用 flate2 自己读 ZIP 目录、解压需要的条目，
//! 再从 XML 里取出文字，不依赖 Office 或其他外部程序。

use std::collections::{BTreeMap, HashMap};
use std::io::Read;

use super::clip;
use super::text::decode_entity;

/// 单个条目解压后的上限，防止压缩炸弹。
const MAX_ENTRY_BYTES: u64 = 64 * 1024 * 1024;
const OUTLINE_SLIDES: usize = 30;
const MAX_SHEET_ROWS: usize = 300;

// ———————————————————————— ZIP ————————————————————————

struct Entry {
    method: u16,
    compressed: usize,
    offset: usize,
}

pub(super) struct Zip {
    data: Vec<u8>,
    entries: BTreeMap<String, Entry>,
}

fn u16_at(data: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(data.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(data.get(at..at + 4)?.try_into().ok()?))
}

impl Zip {
    pub fn open(data: Vec<u8>) -> Result<Self, String> {
        let bad = || "不是有效的 Office 文件（ZIP 结构损坏，或是旧版 .ppt/.doc/.xls 格式）".to_string();
        // 目录结尾记录在文件末尾，后面最多跟 65535 字节的注释
        let floor = data.len().saturating_sub(22 + 65535);
        let end = (floor..=data.len().saturating_sub(22))
            .rev()
            .find(|&at| u32_at(&data, at) == Some(0x0605_4b50))
            .ok_or_else(bad)?;
        let count = u16_at(&data, end + 10).ok_or_else(bad)? as usize;
        let mut at = u32_at(&data, end + 16).ok_or_else(bad)? as usize;
        let mut entries = BTreeMap::new();
        for _ in 0..count {
            if u32_at(&data, at) != Some(0x0201_4b50) {
                return Err(bad());
            }
            let method = u16_at(&data, at + 10).ok_or_else(bad)?;
            let compressed = u32_at(&data, at + 20).ok_or_else(bad)?;
            let name_len = u16_at(&data, at + 28).ok_or_else(bad)? as usize;
            let extra_len = u16_at(&data, at + 30).ok_or_else(bad)? as usize;
            let comment_len = u16_at(&data, at + 32).ok_or_else(bad)? as usize;
            let offset = u32_at(&data, at + 42).ok_or_else(bad)?;
            if compressed == u32::MAX || offset == u32::MAX {
                return Err("文件过大（ZIP64），暂不支持".into());
            }
            let name = String::from_utf8_lossy(data.get(at + 46..at + 46 + name_len).ok_or_else(bad)?).into_owned();
            entries.insert(name, Entry { method, compressed: compressed as usize, offset: offset as usize });
            at += 46 + name_len + extra_len + comment_len;
        }
        Ok(Self { data, entries })
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.entries.keys().map(String::as_str)
    }

    pub fn read(&self, name: &str) -> Result<String, String> {
        let entry = self.entries.get(name).ok_or_else(|| format!("文件里缺少 {name}"))?;
        let bad = || format!("{name} 损坏，无法读取");
        let at = entry.offset;
        if u32_at(&self.data, at) != Some(0x0403_4b50) {
            return Err(bad());
        }
        let start = at + 30 + u16_at(&self.data, at + 26).ok_or_else(bad)? as usize + u16_at(&self.data, at + 28).ok_or_else(bad)? as usize;
        let raw = self.data.get(start..start + entry.compressed).ok_or_else(bad)?;
        let mut bytes = Vec::new();
        match entry.method {
            0 => bytes.extend_from_slice(raw),
            8 => {
                flate2::read::DeflateDecoder::new(raw)
                    .take(MAX_ENTRY_BYTES)
                    .read_to_end(&mut bytes)
                    .map_err(|_| bad())?;
            }
            other => return Err(format!("{name} 使用了不支持的压缩方式 {other}")),
        }
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }
}

// ———————————————————————— XML ————————————————————————

pub(super) enum Token<'a> {
    Open { name: &'a str, attrs: &'a str, empty: bool },
    Close(&'a str),
    Text(String),
}

fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        match rest.find(';').filter(|end| *end <= 10).and_then(|end| Some((end, decode_entity(&rest[1..end])?))) {
            Some((end, decoded)) => {
                out.push_str(&decoded);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// 足够取文字用的 XML 切分：标签、文本；跳过声明与注释，CDATA 当文本。
pub(super) fn tokens(xml: &str) -> Vec<Token<'_>> {
    let mut out = Vec::new();
    let mut rest = xml;
    while !rest.is_empty() {
        let Some(at) = rest.find('<') else {
            out.push(Token::Text(unescape(rest)));
            break;
        };
        if at > 0 {
            out.push(Token::Text(unescape(&rest[..at])));
        }
        rest = &rest[at..];
        if let Some(body) = rest.strip_prefix("<![CDATA[") {
            let end = body.find("]]>").unwrap_or(body.len());
            out.push(Token::Text(body[..end].to_string()));
            rest = body.get(end + 3..).unwrap_or_default();
            continue;
        }
        let (skip_end, skip) = if rest.starts_with("<!--") {
            ("-->", true)
        } else if rest.starts_with("<?") || rest.starts_with("<!") {
            (">", true)
        } else {
            (">", false)
        };
        let Some(end) = rest.find(skip_end) else { break };
        let tag = &rest[1..end];
        rest = &rest[end + skip_end.len()..];
        if skip {
            continue;
        }
        if let Some(name) = tag.strip_prefix('/') {
            out.push(Token::Close(name.trim()));
            continue;
        }
        let empty = tag.ends_with('/');
        let tag = tag.trim_end_matches('/');
        let (name, attrs) = tag.split_once(char::is_whitespace).unwrap_or((tag, ""));
        out.push(Token::Open { name, attrs, empty });
    }
    out
}

pub(super) fn attr(attrs: &str, key: &str) -> Option<String> {
    let mut rest = attrs;
    while let Some(at) = rest.find(key) {
        let before_ok = at == 0 || rest[..at].ends_with(char::is_whitespace);
        let after = rest[at + key.len()..].trim_start();
        if before_ok {
            if let Some(value) = after.strip_prefix('=') {
                let value = value.trim_start();
                let quote = value.chars().next()?;
                if quote == '"' || quote == '\'' {
                    let end = value[1..].find(quote)?;
                    return Some(unescape(&value[1..1 + end]));
                }
            }
        }
        rest = &rest[at + key.len()..];
    }
    None
}

fn tidy(text: &str) -> String {
    text.lines()
        .map(str::trim_end)
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// PowerPoint 的文字在 <a:t>，段落以 </a:p> 结束。
pub(super) fn drawing_text(xml: &str) -> String {
    let mut out = String::new();
    let mut in_text = false;
    for token in tokens(xml) {
        match token {
            Token::Open { name: "a:t", empty: false, .. } => in_text = true,
            Token::Close("a:t") => in_text = false,
            Token::Close("a:p") => out.push('\n'),
            Token::Open { name: "a:br", .. } => out.push('\n'),
            Token::Text(text) if in_text => out.push_str(&text),
            _ => {}
        }
    }
    tidy(&out)
}

/// Word 的文字在 <w:t>，段落 </w:p>，表格单元格之间用 “ | ” 隔开。
pub(super) fn word_text(xml: &str) -> String {
    let mut out = String::new();
    let mut in_text = false;
    for token in tokens(xml) {
        match token {
            Token::Open { name: "w:t", empty: false, .. } => in_text = true,
            Token::Close("w:t") => in_text = false,
            Token::Open { name: "w:tab", .. } => out.push('\t'),
            Token::Open { name: "w:br" | "w:cr", .. } => out.push('\n'),
            Token::Close("w:p") => out.push('\n'),
            Token::Close("w:tc") => {
                if out.ends_with('\n') {
                    out.pop();
                }
                out.push_str(" | ");
            }
            Token::Close("w:tr") => out.push('\n'),
            Token::Text(text) if in_text => out.push_str(&text),
            _ => {}
        }
    }
    tidy(&out)
}

// ———————————————————————— PowerPoint ————————————————————————

fn numbered(names: &[&str], prefix: &str) -> Vec<(usize, String)> {
    let mut found: Vec<(usize, String)> = names
        .iter()
        .filter_map(|name| {
            let number = name.strip_prefix(prefix)?.strip_suffix(".xml")?.parse().ok()?;
            Some((number, name.to_string()))
        })
        .collect();
    found.sort();
    found
}

/// 每页幻灯片的（正文, 讲者备注），按页码顺序。
fn slide_texts(zip: &Zip) -> Result<Vec<(String, String)>, String> {
    let names: Vec<&str> = zip.names().collect();
    let slides = numbered(&names, "ppt/slides/slide");
    if slides.is_empty() {
        return Err("没有找到幻灯片内容".into());
    }
    let mut pages = Vec::new();
    for (number, name) in &slides {
        let text = drawing_text(&zip.read(name)?);
        // 备注页通过关系文件对应到幻灯片
        let rels = zip.read(&format!("ppt/slides/_rels/slide{number}.xml.rels")).unwrap_or_default();
        let notes = tokens(&rels)
            .into_iter()
            .find_map(|token| match token {
                Token::Open { name: "Relationship", attrs, .. } => {
                    let target = attr(attrs, "Target")?;
                    target.contains("notesSlide").then(|| target.replace("../", "ppt/"))
                }
                _ => None,
            })
            .and_then(|path| zip.read(&path).ok())
            .map(|xml| drawing_text(&xml))
            .unwrap_or_default();
        pages.push((text, notes));
    }
    Ok(pages)
}

pub(super) fn read_pptx(zip: &Zip, start: Option<usize>, end: Option<usize>) -> Result<String, String> {
    let pages = slide_texts(zip)?;
    let total = pages.len();
    if start.is_none() && end.is_none() && total > OUTLINE_SLIDES {
        let mut out = format!("共 {total} 页幻灯片，内容较多，以下是每页第一行，可当作目录：\n");
        for (index, (text, _)) in pages.iter().enumerate() {
            let head: String = text.lines().next().unwrap_or("（无文字）").chars().take(50).collect();
            out.push_str(&format!("第 {} 页：{head}\n", index + 1));
        }
        out.push_str("\n请用 start / end 指定页码范围读取全文（含讲者备注）。");
        return Ok(out);
    }
    let first = start.unwrap_or(1).max(1);
    let last = end.unwrap_or(total).min(total);
    if first > last {
        return Err(format!("页码范围无效：共 {total} 页，请求 {first}–{last}"));
    }
    let mut out = format!("共 {total} 页幻灯片，以下为第 {first}–{last} 页：\n");
    for (index, (text, notes)) in pages.iter().enumerate().take(last).skip(first - 1) {
        out.push_str(&format!("\n=== 第 {} 页 ===\n{}\n", index + 1, if text.is_empty() { "（本页没有文字，可能是图片）" } else { text }));
        if !notes.is_empty() {
            out.push_str(&format!("讲者备注：{notes}\n"));
        }
    }
    Ok(out)
}

// ———————————————————————— Excel ————————————————————————

fn shared_strings(zip: &Zip) -> Vec<String> {
    let Ok(xml) = zip.read("xl/sharedStrings.xml") else { return Vec::new() };
    let mut strings = Vec::new();
    let mut current = String::new();
    let (mut in_item, mut in_text, mut in_phonetic) = (false, false, false);
    for token in tokens(&xml) {
        match token {
            Token::Open { name: "si", .. } => {
                in_item = true;
                current.clear();
            }
            Token::Close("si") => {
                in_item = false;
                strings.push(std::mem::take(&mut current));
            }
            // 注音（rPh）里的 <t> 不是单元格内容
            Token::Open { name: "rPh", empty: false, .. } => in_phonetic = true,
            Token::Close("rPh") => in_phonetic = false,
            Token::Open { name: "t", empty: false, .. } => in_text = true,
            Token::Close("t") => in_text = false,
            Token::Text(text) if in_item && in_text && !in_phonetic => current.push_str(&text),
            _ => {}
        }
    }
    strings
}

/// (工作表名, 包内路径)
fn sheets(zip: &Zip) -> Result<Vec<(String, String)>, String> {
    let workbook = zip.read("xl/workbook.xml")?;
    let rels = zip.read("xl/_rels/workbook.xml.rels").unwrap_or_default();
    let targets: HashMap<String, String> = tokens(&rels)
        .into_iter()
        .filter_map(|token| match token {
            Token::Open { name: "Relationship", attrs, .. } => Some((attr(attrs, "Id")?, attr(attrs, "Target")?)),
            _ => None,
        })
        .collect();
    let mut found = Vec::new();
    for token in tokens(&workbook) {
        if let Token::Open { name: "sheet", attrs, .. } = token {
            let (Some(name), Some(id)) = (attr(attrs, "name"), attr(attrs, "r:id")) else { continue };
            let Some(target) = targets.get(&id) else { continue };
            let path = match target.strip_prefix('/') {
                Some(absolute) => absolute.to_string(),
                None => format!("xl/{target}"),
            };
            found.push((name, path));
        }
    }
    Ok(found)
}

fn sheet_rows(xml: &str, strings: &[String]) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut cell_type = String::new();
    let mut value = String::new();
    let (mut in_value, mut in_inline) = (false, false);
    for token in tokens(xml) {
        match token {
            Token::Open { name: "row", .. } => row.clear(),
            Token::Close("row") => rows.push(std::mem::take(&mut row)),
            Token::Open { name: "c", attrs, empty } => {
                cell_type = attr(attrs, "t").unwrap_or_default();
                value.clear();
                if empty {
                    row.push(String::new());
                }
            }
            Token::Close("c") => {
                let shown = match cell_type.as_str() {
                    "s" => value.trim().parse::<usize>().ok().and_then(|index| strings.get(index)).cloned().unwrap_or_default(),
                    "b" => if value.trim() == "1" { "TRUE".into() } else { "FALSE".into() },
                    _ => value.clone(),
                };
                row.push(shown);
            }
            Token::Open { name: "v", empty: false, .. } => in_value = true,
            Token::Close("v") => in_value = false,
            Token::Open { name: "is", .. } => in_inline = true,
            Token::Close("is") => in_inline = false,
            Token::Text(text) if in_value || in_inline => value.push_str(&text),
            _ => {}
        }
    }
    rows
}

pub(super) fn read_xlsx(zip: &Zip, start: Option<usize>, end: Option<usize>) -> Result<String, String> {
    let strings = shared_strings(zip);
    let sheets = sheets(zip)?;
    if sheets.is_empty() {
        return Err("没有找到工作表".into());
    }
    let first = start.unwrap_or(1).max(1);
    let last = end.unwrap_or(sheets.len()).min(sheets.len());
    let names: Vec<&str> = sheets.iter().map(|(name, _)| name.as_str()).collect();
    let mut out = format!("共 {} 个工作表：{}\n", sheets.len(), names.join("、"));
    for (index, (name, path)) in sheets.iter().enumerate().take(last).skip(first - 1) {
        let rows: Vec<Vec<String>> = sheet_rows(&zip.read(path)?, &strings)
            .into_iter()
            .filter(|row| row.iter().any(|cell| !cell.trim().is_empty()))
            .collect();
        out.push_str(&format!("\n=== 工作表 {}：{name}（{} 行有内容）===\n", index + 1, rows.len()));
        for row in rows.iter().take(MAX_SHEET_ROWS) {
            out.push_str(&row.join("\t").trim_end().to_string());
            out.push('\n');
        }
        if rows.len() > MAX_SHEET_ROWS {
            out.push_str(&format!("……其余 {} 行未显示\n", rows.len() - MAX_SHEET_ROWS));
        }
    }
    Ok(out)
}

/// 作业题目识别用：pptx 每页一段（含备注），docx 整篇一段；不是这两种文件时返回 None。
pub(super) fn document_pages(path: &std::path::Path) -> Option<Result<Vec<String>, String>> {
    let extension = path.extension()?.to_string_lossy().to_lowercase();
    if !matches!(extension.as_str(), "docx" | "pptx") {
        return None;
    }
    let read = || -> Result<Vec<String>, String> {
        let data = std::fs::read(path).map_err(|err| format!("读取 {} 失败: {err}", path.display()))?;
        let zip = Zip::open(data)?;
        if extension == "docx" {
            return Ok(vec![word_text(&zip.read("word/document.xml")?)]);
        }
        Ok(slide_texts(&zip)?
            .into_iter()
            .map(|(text, notes)| if notes.is_empty() { text } else { format!("{text}\n{notes}") })
            .collect())
    };
    Some(read())
}

/// 测试用：生成最小的 ZIP（可选 deflate），供其他模块的测试构造 docx/pptx。
#[cfg(test)]
pub(super) fn zip_for_tests(entries: &[(&str, &str, bool)]) -> Vec<u8> {
    tests::make_zip(entries)
}

// ———————————————————————— 工具入口 ————————————————————————

fn index_arg(arguments: &serde_json::Value, key: &str) -> Option<usize> {
    arguments[key].as_u64().map(|value| value as usize).filter(|value| *value > 0)
}

pub(super) async fn read_office(arguments: serde_json::Value) -> Result<String, String> {
    let path = arguments["path"]
        .as_str()
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .ok_or("read_office 需要文件的绝对路径 path")?
        .to_string();
    let extension = std::path::Path::new(&path)
        .extension()
        .map(|ext| ext.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if matches!(extension.as_str(), "ppt" | "doc" | "xls") {
        return Err(format!(".{extension} 是旧版二进制格式，请先另存为 .{extension}x 再读取"));
    }
    if !matches!(extension.as_str(), "pptx" | "docx" | "xlsx") {
        return Err("read_office 只支持 .pptx、.docx、.xlsx；PDF 用 pdf_analysis，纯文本用 read".into());
    }
    let (start, end) = (index_arg(&arguments, "start"), index_arg(&arguments, "end"));
    let text = tokio::task::spawn_blocking(move || -> Result<String, String> {
        let data = std::fs::read(&path).map_err(|err| format!("读取 {path} 失败: {err}"))?;
        let zip = Zip::open(data)?;
        let body = match extension.as_str() {
            "pptx" => read_pptx(&zip, start, end)?,
            "docx" => {
                let text = word_text(&zip.read("word/document.xml")?);
                if text.is_empty() { "（文档里没有文字）".to_string() } else { text }
            }
            _ => read_xlsx(&zip, start, end)?,
        };
        Ok(format!("《{path}》\n{body}"))
    })
    .await
    .map_err(|err| format!("读取任务失败: {err}"))??;
    Ok(clip(&text))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// 测试用的最小 ZIP 写入：可选 deflate 压缩，CRC 填 0（读取端不校验）。
    pub(super) fn make_zip(entries: &[(&str, &str, bool)]) -> Vec<u8> {
        let mut data = Vec::new();
        let mut central = Vec::new();
        for (name, content, deflate) in entries {
            let raw = content.as_bytes();
            let stored: Vec<u8> = if *deflate {
                let mut encoder = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
                encoder.write_all(raw).unwrap();
                encoder.finish().unwrap()
            } else {
                raw.to_vec()
            };
            let method: u16 = if *deflate { 8 } else { 0 };
            let offset = data.len() as u32;
            data.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
            data.extend_from_slice(&[20, 0, 0, 0]);
            data.extend_from_slice(&method.to_le_bytes());
            data.extend_from_slice(&[0; 8]);
            data.extend_from_slice(&(stored.len() as u32).to_le_bytes());
            data.extend_from_slice(&(raw.len() as u32).to_le_bytes());
            data.extend_from_slice(&(name.len() as u16).to_le_bytes());
            data.extend_from_slice(&0u16.to_le_bytes());
            data.extend_from_slice(name.as_bytes());
            data.extend_from_slice(&stored);

            central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
            central.extend_from_slice(&[20, 0, 20, 0, 0, 0]);
            central.extend_from_slice(&method.to_le_bytes());
            central.extend_from_slice(&[0; 8]);
            central.extend_from_slice(&(stored.len() as u32).to_le_bytes());
            central.extend_from_slice(&(raw.len() as u32).to_le_bytes());
            central.extend_from_slice(&(name.len() as u16).to_le_bytes());
            central.extend_from_slice(&[0; 12]);
            central.extend_from_slice(&offset.to_le_bytes());
            central.extend_from_slice(name.as_bytes());
        }
        let central_offset = data.len() as u32;
        data.extend_from_slice(&central);
        data.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
        data.extend_from_slice(&[0; 4]);
        data.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        data.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        data.extend_from_slice(&(central.len() as u32).to_le_bytes());
        data.extend_from_slice(&central_offset.to_le_bytes());
        data.extend_from_slice(&[0; 2]);
        data
    }

    #[test]
    fn pptx_slides_come_in_numeric_order_with_speaker_notes() {
        let slide = |text: &str| format!(r#"<?xml version="1.0"?><p:sld><p:txBody><a:p><a:r><a:t>{text}</a:t></a:r></a:p><a:p><a:r><a:t>第二行 &amp; 公式 a&lt;b</a:t></a:r></a:p></p:txBody></p:sld>"#);
        let (s1, s2, s10) = (slide("第一讲 概率"), slide("条件概率"), slide("总结"));
        let zip = Zip::open(make_zip(&[
            ("ppt/slides/slide10.xml", &s10, true),
            ("ppt/slides/slide2.xml", &s2, false),
            ("ppt/slides/slide1.xml", &s1, true),
            ("ppt/slides/_rels/slide2.xml.rels", r#"<Relationships><Relationship Id="rId2" Type="x/notesSlide" Target="../notesSlides/notesSlide7.xml"/></Relationships>"#, true),
            ("ppt/notesSlides/notesSlide7.xml", "<p:notes><a:p><a:r><a:t>这里要强调独立性</a:t></a:r></a:p></p:notes>", true),
        ]))
        .unwrap();
        let text = read_pptx(&zip, None, None).unwrap();
        let (one, two, ten) = (text.find("第一讲 概率").unwrap(), text.find("条件概率").unwrap(), text.find("总结").unwrap());
        assert!(one < two && two < ten, "按页码数字排序，不是字典序：{text}");
        assert!(text.contains("=== 第 2 页 ===\n条件概率\n第二行 & 公式 a<b\n讲者备注：这里要强调独立性"), "{text}");
        let part = read_pptx(&zip, Some(3), Some(3)).unwrap();
        assert!(part.contains("=== 第 3 页 ===\n总结") && !part.contains("条件概率"), "{part}");
        assert!(read_pptx(&zip, Some(5), Some(4)).is_err());
    }

    #[test]
    fn docx_keeps_paragraphs_tabs_and_tables() {
        let xml = r#"<w:document><w:body>
            <w:p><w:r><w:t>实验报告</w:t></w:r></w:p>
            <w:p><w:r><w:t xml:space="preserve">姓名：</w:t></w:r><w:r><w:tab/><w:t>张三</w:t></w:r></w:p>
            <w:tbl><w:tr><w:tc><w:p><w:r><w:t>电压</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>3.3V</w:t></w:r></w:p></w:tc></w:tr></w:tbl>
            </w:body></w:document>"#;
        let zip = Zip::open(make_zip(&[("word/document.xml", xml, true)])).unwrap();
        let text = word_text(&zip.read("word/document.xml").unwrap());
        assert_eq!(text, "实验报告\n姓名：\t张三\n电压 | 3.3V |");
    }

    #[test]
    fn xlsx_resolves_shared_strings_inline_strings_and_sheet_names() {
        let zip = Zip::open(make_zip(&[
            ("xl/workbook.xml", r#"<workbook><sheets><sheet name="成绩" sheetId="1" r:id="rId1"/><sheet name="说明" sheetId="2" r:id="rId2"/></sheets></workbook>"#, true),
            ("xl/_rels/workbook.xml.rels", r#"<Relationships><Relationship Id="rId1" Target="worksheets/sheet1.xml"/><Relationship Id="rId2" Target="/xl/worksheets/sheet2.xml"/></Relationships>"#, true),
            ("xl/sharedStrings.xml", "<sst><si><t>姓名</t></si><si><r><t>分</t></r><r><t>数</t></r><rPh><t>fen</t></rPh></si><si><t>张三</t></si></sst>", true),
            ("xl/worksheets/sheet1.xml", r#"<worksheet><sheetData><row r="1"><c r="A1" t="s"><v>0</v></c><c r="B1" t="s"><v>1</v></c></row><row r="2"><c r="A2" t="s"><v>2</v></c><c r="B2"><v>93.5</v></c><c r="C2" t="b"><v>1</v></c></row><row r="3"><c r="A3"/></row></sheetData></worksheet>"#, true),
            ("xl/worksheets/sheet2.xml", r#"<worksheet><sheetData><row><c t="inlineStr"><is><t>按学号排序</t></is></c></row></sheetData></worksheet>"#, false),
        ]))
        .unwrap();
        let text = read_xlsx(&zip, None, None).unwrap();
        assert!(text.contains("共 2 个工作表：成绩、说明"), "{text}");
        assert!(text.contains("=== 工作表 1：成绩（2 行有内容）===\n姓名\t分数\n张三\t93.5\tTRUE\n"), "{text}");
        assert!(text.contains("=== 工作表 2：说明（1 行有内容）===\n按学号排序"), "{text}");
    }

    #[test]
    fn broken_and_legacy_files_get_clear_errors() {
        assert!(Zip::open(b"%PDF-1.7 not a zip".to_vec()).err().unwrap().contains("不是有效的 Office 文件"));
        let zip = Zip::open(make_zip(&[("a.txt", "x", false)])).unwrap();
        assert!(zip.read("word/document.xml").unwrap_err().contains("缺少"));
        assert_eq!(attr(r#"Id="rId1" Target='a&amp;b.xml'"#, "Target").as_deref(), Some("a&b.xml"));
        assert_eq!(attr(r#"r:id="rId3""#, "id"), None, "不能把 r:id 当成 id");
    }

    #[tokio::test]
    async fn unsupported_extensions_are_rejected_before_reading() {
        let error = read_office(serde_json::json!({"path": "/tmp/a.ppt"})).await.unwrap_err();
        assert!(error.contains("旧版"), "{error}");
        let error = read_office(serde_json::json!({"path": "/tmp/a.pdf"})).await.unwrap_err();
        assert!(error.contains("pdf_analysis"), "{error}");
    }
}
