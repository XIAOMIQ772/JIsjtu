use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::{Arc, Mutex, OnceLock};

use super::clip;

const PDF_SHORT_PAGES: usize = 12;
const PDF_PAGE_HEAD_CHARS: usize = 60;

struct CachedPdf {
    digest: u64,
    pages: Arc<Vec<String>>,
}

fn pdf_cache() -> &'static Mutex<HashMap<String, CachedPdf>> {
    static CACHE: OnceLock<Mutex<HashMap<String, CachedPdf>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

#[cfg(target_os = "macos")]
pub(super) fn extract_pages(bytes: &[u8]) -> Result<Vec<String>, String> {
    use std::ffi::{CStr, c_char};

    unsafe extern "C" {
        fn jisjtu_pdf_pages(bytes: *const u8, length: usize, error: *mut *mut c_char) -> *mut c_char;
    }
    let mut error = std::ptr::null_mut();
    // The bridge returns a malloc-owned UTF-8 JSON array or a malloc-owned error.
    unsafe {
        let pages = jisjtu_pdf_pages(bytes.as_ptr(), bytes.len(), &mut error);
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
pub(super) fn extract_pages(bytes: &[u8]) -> Result<Vec<String>, String> {
    pdf_extract::extract_text_from_mem_by_pages(bytes).map_err(|err| err.to_string())
}

pub(super) async fn load_pdf_pages(path: &str) -> Result<Arc<Vec<String>>, String> {
    let owned = path.to_string();
    tokio::task::spawn_blocking(move || cached_pages(&owned))
        .await
        .map_err(|err| format!("解析任务失败: {err}"))?
}

fn cached_pages(path: &str) -> Result<Arc<Vec<String>>, String> {
    // 指纹与解析使用同一份快照，即使在读取期间替换同名文件也不会混用两个版本。
    let bytes = std::fs::read(path).map_err(|err| format!("读取 PDF 失败: {err}"))?;
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    let digest = hasher.finish();
    if let Some(hit) = pdf_cache().lock().unwrap().get(path).filter(|hit| hit.digest == digest) {
        return Ok(hit.pages.clone());
    }

    let pages = extract_pages(&bytes).map_err(|err| format!("解析 PDF《{path}》失败：{err}"))?;

    if pages.iter().all(|page| page.trim().is_empty()) {
        #[cfg(target_os = "macos")]
        return Err(format!("PDF《{path}》没有识别到可读取的正文（共 {} 页）", pages.len()));
        #[cfg(not(target_os = "macos"))]
        return Err("没有可提取的文字，扫描版 PDF 的离线 OCR 目前仅支持 macOS".into());
    }

    let pages = Arc::new(pages);
    pdf_cache()
        .lock()
        .unwrap()
        .insert(path.to_string(), CachedPdf { digest, pages: pages.clone() });
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
    if let Some(value) = arguments.get("image_page").filter(|value| !value.is_null()) {
        return super::pdf_page_view::inspect(path, value).await;
    }

    let pages = load_pdf_pages(&path).await?;
    let total = pages.len();
    let start = page_arg(&arguments, "start_page");
    let end = page_arg(&arguments, "end_page");

    if start.is_none() && end.is_none() && total > PDF_SHORT_PAGES {
        let mut text = format!("《{path}》共 {total} 页，内容较长。以下是各页开头，可当作目录：\n");
        for (index, page) in pages.iter().enumerate() {
            let (body, notice) = page.rsplit_once('\n')
                .filter(|(_, last)| last.starts_with("[第 ") && last.contains("image_page="))
                .unwrap_or((page, ""));
            let head: String = body
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .chars()
                .take(PDF_PAGE_HEAD_CHARS)
                .collect();
            if head.is_empty() && notice.is_empty() {
                continue;
            }
            text.push_str(&format!("第 {} 页：{head}{notice}\n", index + 1));
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
