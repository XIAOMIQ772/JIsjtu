//! The Windows distribution ships PDFium and the Chinese/English Tesseract models together.
use std::io::{BufWriter, Seek, Write};
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::OnceLock;

use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use pdfium_render::prelude::*;

fn tools_dir() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("AGENT_PDF_TOOLS_DIR") {
        return Ok(std::path::absolute(path)?);
    }
    let exe = std::env::current_exe()?;
    Ok(exe.parent().context("无法定位程序目录")?.join("pdf-tools"))
}

fn pdfium() -> Result<&'static Pdfium, String> {
    static PDFIUM: OnceLock<Result<Pdfium, String>> = OnceLock::new();
    PDFIUM
        .get_or_init(|| {
            let dll = tools_dir().map_err(|e| e.to_string())?.join("pdfium.dll");
            Pdfium::bind_to_library(&dll).map(Pdfium::new).map_err(|e| {
                format!(
                    "加载 {} 失败，请重新安装完整的 Windows 包：{e}",
                    dll.display()
                )
            })
        })
        .as_ref()
        .map_err(Clone::clone)
}

fn render<'a>(page: &'a PdfPage, long_edge: i32) -> Result<PdfBitmap<'a>> {
    let config = PdfRenderConfig::new()
        .set_target_width(long_edge)
        .set_maximum_height(long_edge);
    let bitmap = page.render_with_config(&config)?;
    ensure!(
        bitmap.width() > 0 && bitmap.height() > 0,
        "PDF 页面尺寸无效"
    );
    Ok(bitmap)
}

pub(super) fn page_jpeg(path: &str, number: usize) -> Result<String, String> {
    let api = pdfium()?;
    let image = || -> Result<String> {
        // Rust opens the path so non-ASCII Windows filenames work with any Pdfium C runtime.
        let file = std::fs::File::open(path)?;
        let doc = api.load_pdf_from_reader(file, None)?;
        ensure!(
            (1..=doc.pages().len() as usize).contains(&number),
            "页码超出范围"
        );
        let page = doc.pages().get((number - 1) as PdfPageIndex)?;
        let bitmap = render(&page, 1700)?;
        let mut jpeg = Vec::new();
        jpeg_encoder::Encoder::new(&mut jpeg, 85).encode(
            &bitmap.as_rgba_bytes(),
            bitmap.width() as u16,
            bitmap.height() as u16,
            jpeg_encoder::ColorType::Rgba,
        )?;
        Ok(format!("data:image/jpeg;base64,{}", STANDARD.encode(jpeg)))
    };
    image().map_err(|err| format!("无法渲染 PDF 第 {number} 页：{err:#}"))
}

fn recognize(page: &PdfPage) -> Result<String> {
    let dir = tools_dir()?;
    let bitmap = render(page, 2400)?;
    // A seekable stdin avoids command-line filenames and pipe deadlocks on large scans.
    let mut image = tempfile::tempfile()?;
    {
        let mut writer = BufWriter::new(&mut image);
        write!(writer, "P5\n{} {}\n255\n", bitmap.width(), bitmap.height())?;
        let gray: Vec<u8> = bitmap
            .as_rgba_bytes()
            .chunks_exact(4)
            .map(|p| ((77 * p[0] as u32 + 150 * p[1] as u32 + 29 * p[2] as u32) >> 8) as u8)
            .collect();
        writer.write_all(&gray)?;
        writer.flush()?;
    }
    image.rewind()?;
    let result = Command::new(dir.join("tesseract.exe"))
        .current_dir(dir)
        .args([
            "stdin",
            "stdout",
            "--tessdata-dir",
            "tessdata",
            "-l",
            "chi_sim+eng",
            "--dpi",
            "200",
        ])
        .creation_flags(0x08000000) // CREATE_NO_WINDOW
        .stdin(Stdio::from(image))
        .output()
        .context("无法启动安装包中的 Tesseract")?;
    ensure!(
        result.status.success(),
        "Tesseract：{}",
        String::from_utf8_lossy(&result.stderr).trim()
    );
    Ok(String::from_utf8(result.stdout)?)
}

fn extract(doc: &PdfDocument) -> Result<Vec<String>> {
    let mut pages = Vec::new();
    let mut any_text = false;
    let mut first_error = None;
    for index in doc.pages().as_range() {
        let number = index + 1;
        let page = doc.pages().get(index)?;
        let mut text = page.text().map(|layer| layer.all()).unwrap_or_default();
        let needs_ocr = text.trim().is_empty()
            || !page.annotations().is_empty()
            || page
                .objects()
                .iter()
                .any(|object| object.object_type() != PdfPageObjectType::Text);
        if needs_ocr {
            // Keep exact native text (including equations). Mask its page objects only
            // in this in-memory document, leaving images between text runs visible.
            if !text.trim().is_empty() {
                for mut object in page.objects().iter() {
                    if object
                        .as_text_object()
                        .is_some_and(|o| !o.text().trim().is_empty())
                    {
                        let _ = object.set_inactive();
                    }
                }
            }
            let notice = match recognize(&page) {
                Ok(ocr) => {
                    let compact = |s: &str| s.split_whitespace().collect::<String>().to_lowercase();
                    let native = compact(&text);
                    for line in ocr.lines().map(str::trim).filter(|line| !line.is_empty()) {
                        if !native.contains(&compact(line)) {
                            if !text.is_empty() {
                                text.push('\n');
                            }
                            text.push_str(line);
                        }
                    }
                    "含图像或图形，请核对公式与布局".to_owned()
                }
                Err(error) => {
                    let reason = format!("离线 OCR 失败：{error:#}");
                    first_error.get_or_insert(reason.clone());
                    reason
                }
            };
            any_text |= !text.trim().is_empty();
            text.push_str(&format!(
                "\n[第 {number} 页{notice}。查看原页可用 pdf_analysis 的 image_page={number}]"
            ));
        } else {
            any_text = true;
        }
        pages.push(text);
    }
    ensure!(
        any_text,
        "{}。可用 image_page 查看指定页原图。",
        first_error.unwrap_or_else(|| "没有识别到文字".into())
    );
    Ok(pages)
}

pub(super) fn extract_pages(bytes: &[u8]) -> Result<Vec<String>, String> {
    let doc = pdfium()?
        .load_pdf_from_byte_slice(bytes, None)
        .map_err(|e| e.to_string())?;
    extract(&doc).map_err(|e| format!("{e:#}"))
}

pub(super) fn page_count(bytes: &[u8]) -> usize {
    pdfium()
        .ok()
        .and_then(|api| api.load_pdf_from_byte_slice(bytes, None).ok())
        .map_or(0, |doc| doc.pages().len() as usize)
}
