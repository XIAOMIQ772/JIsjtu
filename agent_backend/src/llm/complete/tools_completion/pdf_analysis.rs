use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use super::clip;

const PDF_SHORT_PAGES: usize = 12;
const PDF_PAGE_HEAD_CHARS: usize = 60;

fn pdf_cache() -> &'static Mutex<HashMap<String, Arc<Vec<String>>>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Arc<Vec<String>>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

#[cfg(target_os = "macos")]
fn extract_pages(path: String) -> Result<Vec<String>, String> {
    use std::ffi::{CStr, CString, c_char};

    unsafe extern "C" {
        fn jisjtu_pdf_pages(path: *const c_char, error: *mut *mut c_char) -> *mut c_char;
    }
    let path = CString::new(path).map_err(|_| "文件路径包含空字符")?;
    let mut error = std::ptr::null_mut();
    // The bridge returns a malloc-owned UTF-8 JSON array or a malloc-owned error.
    unsafe {
        let pages = jisjtu_pdf_pages(path.as_ptr(), &mut error);
        if pages.is_null() {
            if error.is_null() {
                return Err("系统未能读取 PDF".into());
            }
            let message = CStr::from_ptr(error).to_string_lossy().into_owned();
            libc::free(error.cast());
            return Err(message);
        }
        let parsed = serde_json::from_slice(CStr::from_ptr(pages).to_bytes());
        libc::free(pages.cast());
        parsed.map_err(|err| err.to_string())
    }
}

#[cfg(not(target_os = "macos"))]
fn extract_pages(path: String) -> Result<Vec<String>, String> {
    pdf_extract::extract_text_by_pages(path).map_err(|err| err.to_string())
}

pub(super) async fn load_pdf_pages(path: &str) -> Result<Arc<Vec<String>>, String> {
    {
        let cache = pdf_cache().lock().unwrap();
        if let Some(pages) = cache.get(path) {
            return Ok(pages.clone());
        }
    }

    let owned = path.to_string();
    let pages = tokio::task::spawn_blocking(move || extract_pages(owned))
        .await
        .map_err(|err| format!("解析任务失败: {err}"))?
        .map_err(|err| format!("解析 PDF 失败: {err}"))?;

    if pages.iter().all(|page| page.trim().is_empty()) {
        #[cfg(target_os = "macos")]
        return Err("PDF 中未识别出文字（已尝试离线 OCR）".into());
        #[cfg(not(target_os = "macos"))]
        return Err("没有可提取的文字，扫描版 PDF 的离线 OCR 目前仅支持 macOS".into());
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
