use std::path::{Path, PathBuf};

use super::super::office::{Token, Zip, tokens, word_text};
use super::super::pdf_analysis::extract_pages;
use super::split_questions;

pub(super) fn check_required_files(arguments: &serde_json::Value, files: &[PathBuf]) -> Result<(), String> {
    let Some(required) = arguments.get("required_files").filter(|v| !v.is_null()) else { return Ok(()) };
    let items = required.as_array().ok_or("required_files 应为老师明确要求单独上传的文件清单。")?;
    let mut missing = Vec::new();
    for item in items {
        let item = item.as_str().map(str::trim).filter(|s| !s.is_empty()).ok_or("必交文件名不能为空。")?;
        let found = files.iter().any(|file| match item.strip_prefix('.') {
            Some(extension) => file.extension().is_some_and(|ext| ext.eq_ignore_ascii_case(extension)),
            None => file.file_name().is_some_and(|name| name == item),
        });
        if !found {
            missing.push(item);
        }
    }
    if !missing.is_empty() {
        return Err(format!("缺少要求单独上传的文件：{}。请补齐后再提交。", missing.join("、")));
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn pdf_page_count(path: &Path, _: &[u8]) -> usize {
    use std::ffi::{CString, c_char};
    unsafe extern "C" {
        fn jisjtu_pdf_page_count(path: *const c_char) -> usize;
    }
    CString::new(path.as_os_str().as_encoded_bytes())
        .map_or(0, |name| unsafe { jisjtu_pdf_page_count(name.as_ptr()) })
}

#[cfg(not(target_os = "macos"))]
fn pdf_page_count(_: &Path, bytes: &[u8]) -> usize {
    pdf_extract::Document::load_mem(bytes)
        .ok()
        .filter(|pdf| !pdf.is_encrypted())
        .map_or(0, |pdf| pdf.page_iter().count())
}

pub(super) struct Inspection {
    summary: String,
    labels: Vec<String>,
}

impl std::fmt::Display for Inspection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.summary)?;
        if !self.labels.is_empty() {
            write!(f, "，可读题号：{}", self.labels.join("、"))?;
        }
        Ok(())
    }
}

fn answer_labels(pages: &[String]) -> Vec<String> {
    // Markdown headings and emphasis around a question number disappear when rendered.
    let visible: Vec<_> = pages
        .iter()
        .map(|page| {
            page.lines()
                .map(|line| {
                    let stripped = line.trim_start_matches('#');
                    let line = if stripped.len() < line.len() && stripped.starts_with(char::is_whitespace) {
                        stripped.trim_start()
                    } else {
                        line
                    };
                    line.trim_start_matches("**")
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .collect();
    let parsed = split_questions(&visible);
    parsed.questions.into_iter().filter(|q| q.label != "全文").map(|q| q.label).collect()
}

fn word_labels(bytes: Vec<u8>) -> Result<Vec<String>, String> {
    let xml = Zip::open(bytes)?.read("word/document.xml")?;
    let mut stack = Vec::new();
    let mut document = false;
    let mut body = false;
    for token in tokens(&xml) {
        match token {
            Token::Open { name, empty, .. } => {
                let local = name.rsplit(':').next().unwrap_or(name);
                if stack.is_empty() {
                    if document || local != "document" {
                        return Err("Word 主文档的根元素无效".into());
                    }
                    document = true;
                }
                if stack.len() == 1 && local == "body" {
                    body = true;
                }
                if !empty {
                    stack.push(name);
                }
            }
            Token::Close(name) => {
                if stack.pop() != Some(name) {
                    return Err("Word 主文档结构不完整".into());
                }
            }
            _ => {}
        }
    }
    if !document || !body || !stack.is_empty() {
        return Err("Word 文件缺少完整的主文档正文".into());
    }
    Ok(answer_labels(&[word_text(&xml)]))
}

/// Read the staged file that will be sent to Canvas, including image-only PDFs.
pub(super) fn inspect(path: &Path) -> Result<Inspection, String> {
    let size = std::fs::metadata(path).map_err(|e| format!("无法读取 {}：{e}", path.display()))?.len();
    if size == 0 {
        return Err(format!("{} 是空文件，请重新生成。", path.display()));
    }
    let ext = path.extension().unwrap_or_default().to_string_lossy().to_ascii_lowercase();
    let read = || std::fs::read(path).map_err(|e| e.to_string());
    let (details, labels) = (|| -> Result<(String, Vec<String>), String> {
        match ext.as_str() {
            "pdf" => {
                let bytes = read()?;
                let count = pdf_page_count(path, &bytes);
                if count == 0 {
                    return Err("PDF 无法打开、未解锁或没有有效页面".into());
                }
                let labels = extract_pages(&bytes).map(|pages| answer_labels(&pages)).unwrap_or_default();
                Ok((format!("PDF 共 {count} 页"), labels))
            }
            "docx" => Ok(("Word 主文档可读".into(), word_labels(read()?)?)),
            "txt" | "md" => Ok(match std::str::from_utf8(&read()?) {
                Ok(s) => ("文本可读".into(), answer_labels(&[s.to_string()])),
                Err(_) => ("文本文件未按 UTF-8 编码".into(), Vec::new()),
            }),
            _ => Ok((String::new(), Vec::new())),
        }
    })()
    .map_err(|error| format!("{} 未通过交前检查：{error}", path.display()))?;
    let summary =
        if details.is_empty() { format!("{size} 字节") } else { format!("{size} 字节，{details}") };
    Ok(Inspection { summary, labels })
}

/// Labels may use different headings in the question and the answer (e.g. “Problem 2” and “2.”).
fn label_key(label: &str) -> String {
    let label: String = label.chars().filter(|c| !c.is_whitespace()).map(|c| c.to_ascii_lowercase()).collect();
    let label = label.trim_matches(['(', '（', ')', '）', '.', '．', '、', ':', '：']);
    if let Some(at) = label.find(|c: char| c.is_ascii_digit()) {
        let number = &label[at..];
        if number.trim_end_matches(['题', '问']).chars().all(|c| c.is_ascii_digit() || c == '.') {
            return number.trim_end_matches(['题', '问']).to_owned();
        }
    }
    let raw = label.trim_start_matches('第').trim_end_matches(['题', '问']);
    let digit = |s: &str| {
        "零一二三四五六七八九".chars().position(|c| Some(c) == s.chars().next() && s.chars().count() == 1)
    };
    let chinese = if let Some((tens, ones)) = raw.split_once('十') {
        let tens = if tens.is_empty() { Some(1) } else { digit(tens) };
        let ones = if ones.is_empty() { Some(0) } else { digit(ones) };
        tens.zip(ones).map(|(t, o)| 10 * t + o)
    } else {
        digit(raw)
    };
    chinese.map_or_else(|| raw.to_owned(), |n| n.to_string())
}

pub(super) fn check_required_questions<'a>(
    args: &serde_json::Value,
    text: Option<&str>,
    files: impl Iterator<Item = &'a Inspection>,
) -> Result<(), String> {
    let Some(required) = args.get("required_questions").filter(|v| !v.is_null()) else { return Ok(()) };
    let items = required.as_array().ok_or("required_questions 应为要求在成品中标出的原题号列表。")?;
    let mut labels = Vec::new();
    if let Some(text) = text {
        let visible =
            if text.trim_start().starts_with('<') { super::html_to_text(text) } else { text.to_owned() };
        labels.extend(answer_labels(&[visible]));
    }
    labels.extend(files.flat_map(|file| file.labels.iter().cloned()));
    let mut available: Vec<_> = labels.iter().map(|label| label_key(label)).collect();
    let mut missing = Vec::new();
    for item in items {
        let label = item.as_str().map(str::trim).filter(|s| !s.is_empty()).ok_or("必答题号不能为空。")?;
        let key = label_key(label);
        if let Some(position) = available.iter().position(|v| *v == key) {
            available.swap_remove(position);
        } else {
            missing.push(label);
        }
    }
    if missing.is_empty() {
        return Ok(());
    }
    let found = if labels.is_empty() { "无明确题号".to_owned() } else { labels.join("、") };
    Err(format!("最终成品中未核实题号 {}（实际读到：{found}）。请按老师的编号核对成品。", missing.join("、")))
}
