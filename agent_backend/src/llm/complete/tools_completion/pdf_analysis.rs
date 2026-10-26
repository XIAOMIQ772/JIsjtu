use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use super::clip;

const PDF_SHORT_PAGES: usize = 12;
const PDF_PAGE_HEAD_CHARS: usize = 60;

fn pdf_cache() -> &'static Mutex<HashMap<String, Arc<Vec<String>>>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Arc<Vec<String>>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(super) async fn load_pdf_pages(path: &str) -> Result<Arc<Vec<String>>, String> {
    {
        let cache = pdf_cache().lock().unwrap();
        if let Some(pages) = cache.get(path) {
            return Ok(pages.clone());
        }
    }

    let owned = path.to_string();
    let pages = tokio::task::spawn_blocking(move || pdf_extract::extract_text_by_pages(owned))
        .await
        .map_err(|err| format!("解析任务失败: {err}"))?
        .map_err(|err| format!("解析 PDF 失败: {err}"))?;

    if pages.iter().all(|page| page.trim().is_empty()) {
        return Err("没有任何文字，可能是扫描版 PDF，需要 OCR".to_string());
    }

    let pages = Arc::new(pages);
    pdf_cache()
        .lock()
        .unwrap()
        .insert(path.to_string(), pages.clone());
    Ok(pages)
}

fn page_arg(arguments: &serde_json::Value, key: &str) -> Option<usize> {
    let value = arguments.get(key)?;
    let parsed = value
        .as_u64()
        .map(|number| number as usize)
        .or_else(|| value.as_str()?.trim().parse::<usize>().ok())?;
    (parsed > 0).then_some(parsed)
}

pub(super) async fn pdf_analysis(arguments: serde_json::Value) -> Result<String, String> {
    let path = arguments["path"]
        .as_str()
        .ok_or_else(|| "pdf_analysis 缺少 path 参数".to_string())?
        .to_string();

    let pages = load_pdf_pages(&path).await?;
    let total = pages.len();
    let start = page_arg(&arguments, "start_page");
    let end = page_arg(&arguments, "end_page");

    if start.is_none() && end.is_none() && total > PDF_SHORT_PAGES {
        let mut text = format!("《{path}》共 {total} 页，内容较长。以下是各页开头，可当作目录：\n");
        for (index, page) in pages.iter().enumerate() {
            let head: String = page
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .chars()
                .take(PDF_PAGE_HEAD_CHARS)
                .collect();
            if head.is_empty() {
                continue;
            }
            text.push_str(&format!("第 {} 页：{head}\n", index + 1));
        }
        text.push_str("\n请确定需要的页码，再用 start_page / end_page 调用本工具读取原文。");
        return Ok(clip(&text));
    }

    let start = start.unwrap_or(1);
    let end = end.unwrap_or(total);
    if start > total || end > total || end < start {
        return Err(format!(
            "页范围无效：该 PDF 共 {total} 页，请求的是 {start}–{end}"
        ));
    }

    let mut text = format!("《{path}》第 {start}–{end} 页（全文共 {total} 页）：\n");
    for (index, page) in pages[start - 1..end].iter().enumerate() {
        let page = page.trim();
        if page.is_empty() {
            continue;
        }
        text.push_str(&format!("\n--- 第 {} 页 ---\n{page}\n", start + index));
    }

    Ok(clip(&text))
}
