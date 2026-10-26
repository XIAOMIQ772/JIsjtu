use std::path::{Path, PathBuf};

use super::clip;

const FILES_MAX_PAGES: usize = 10;

#[derive(serde::Deserialize)]
pub(super) struct CanvasFile {
    id: u64,
    display_name: String,
    #[serde(default)]
    filename: Option<String>,
    #[serde(default, rename = "content-type")]
    pub(super) content_type: Option<String>,
    #[serde(default)]
    size: Option<u64>,
    #[serde(default)]
    url: Option<String>,
}

fn percent_decode(text: &str) -> String {
    if !text.contains('%') {
        return text.to_string();
    }

    let bytes = text.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut index = 0;

    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && bytes[index + 1].is_ascii_hexdigit()
            && bytes[index + 2].is_ascii_hexdigit()
        {
            let byte = u8::from_str_radix(&text[index + 1..index + 3], 16).unwrap_or(bytes[index]);
            out.push(byte);
            index += 3;
        } else {
            if bytes[index] == b'+' {
                out.push(b' ');
            } else {
                out.push(bytes[index]);
            }
            index += 1;
        }
    }

    String::from_utf8_lossy(&out).into_owned()
}

impl CanvasFile {
    pub(super) fn name(&self) -> &str {
        if self.display_name.trim().is_empty() {
            self.filename.as_deref().unwrap_or("(未命名)")
        } else {
            &self.display_name
        }
    }

    /// 落盘用的文件名。Canvas 对非 ASCII 名字会做百分号编码
    /// （如 "01.+%E8%AF%BE%E7%A8%8B%E7%AE%80%E4%BB%8B.pdf"），直接当路径会写出一串乱码。
    pub(super) fn local_name(&self) -> String {
        let decoded = percent_decode(self.name());
        Path::new(&decoded)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| format!("canvas-file-{}", self.id))
    }

    fn human_size(&self) -> String {
        match self.size {
            Some(size) if size >= 1024 * 1024 => format!("{:.1} MB", size as f64 / 1048576.0),
            Some(size) if size >= 1024 => format!("{:.0} KB", size as f64 / 1024.0),
            Some(size) => format!("{size} B"),
            None => "大小未知".to_string(),
        }
    }
}

fn page_url(base: &str, path: &str, page: usize) -> String {
    format!("{base}{path}?per_page=100&page={page}")
}

fn next_link(header: &str) -> Option<String> {
    header.split(',').find_map(|part| {
        let mut segments = part.split(';');
        let url = segments.next()?.trim();
        if segments.any(|s| s.trim() == "rel=\"next\"") {
            Some(
                url.trim_start_matches('<')
                    .trim_end_matches('>')
                    .to_string(),
            )
        } else {
            None
        }
    })
}

async fn fetch_course_files(
    client: &reqwest::Client,
    token: &str,
    course_id: &str,
) -> Result<Vec<CanvasFile>, String> {
    let base = "https://oc.sjtu.edu.cn";
    let path = format!("/api/v1/courses/{course_id}/files");
    let mut url = page_url(base, &path, 1);
    let mut files = Vec::new();

    for _ in 0..FILES_MAX_PAGES {
        let response = client
            .get(&url)
            .bearer_auth(token)
            .send()
            .await
            .map_err(|err| format!("请求 Canvas 失败: {err}"))?;

        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err("认证失败 (401)：令牌无效或已过期".to_string());
        }
        if status == reqwest::StatusCode::NOT_FOUND {
            return Err(format!("课程 {course_id} 不存在，或该课程未启用「文件」页面"));
        }
        if !status.is_success() {
            return Err(format!("Canvas 返回 {status}"));
        }

        let next = response
            .headers()
            .get("link")
            .and_then(|value| value.to_str().ok())
            .and_then(next_link);

        let batch: Vec<CanvasFile> = response
            .json()
            .await
            .map_err(|err| format!("解析文件列表失败: {err}"))?;
        let empty = batch.is_empty();
        files.extend(batch);

        match next {
            Some(next_url) if !empty => url = next_url,
            _ => break,
        }
    }

    Ok(files)
}

fn pick_file<'a>(files: &'a [CanvasFile], target: &str) -> Result<&'a CanvasFile, String> {
    if let Ok(id) = target.parse::<u64>() {
        return files
            .iter()
            .find(|file| file.id == id)
            .ok_or_else(|| format!("课程里没有 id 为 {id} 的文件"));
    }

    let needle = target.to_lowercase();
    let matched: Vec<&CanvasFile> = files
        .iter()
        .filter(|file| {
            file.name().to_lowercase().contains(&needle)
                || file.display_name.to_lowercase().contains(&needle)
        })
        .collect();

    match matched.len() {
        0 => Err(format!("没有匹配「{target}」的文件，可先用 action=list 查看")),
        1 => Ok(matched[0]),
        count => {
            let mut text = format!("匹配到 {count} 个文件，请用更精确的名字或文件 id：\n");
            for file in matched {
                text.push_str(&format!("  [{}] {}\n", file.id, file.display_name));
            }
            Err(text)
        }
    }
}

fn save_target(file: &CanvasFile, requested: Option<&str>) -> PathBuf {
    if let Some(path) = requested {
        if !path.is_empty() {
            return PathBuf::from(path);
        }
    }

    let dir = std::env::var("HOME")
        .map(|home| PathBuf::from(home).join("Downloads"))
        .unwrap_or_else(|_| PathBuf::from("."));

    dir.join(file.local_name())
}

fn open_with_system(path: &Path) -> Result<(), String> {
    let target = path.to_string_lossy().into_owned();
    let (program, args): (&str, Vec<String>) = if cfg!(target_os = "macos") {
        ("open", vec![target])
    } else if cfg!(target_os = "windows") {
        (
            "cmd",
            vec!["/C".to_string(), "start".to_string(), String::new(), target],
        )
    } else {
        ("xdg-open", vec![target])
    };

    std::process::Command::new(program)
        .args(&args)
        .spawn()
        .map(|_| ())
        .map_err(|err| format!("调用 {program} 失败: {err}"))
}

pub(super) async fn download_file(
    client: &reqwest::Client,
    token: &str,
    file: &CanvasFile,
    target: &Path,
) -> Result<String, String> {
    if let Some(parent) = target.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|err| err.to_string())?;
        }
    }

    let url = file
        .url
        .as_deref()
        .ok_or_else(|| format!("文件 {} 没有提供下载地址", file.display_name))?;

    let response = client
        .get(url)
        .bearer_auth(token)
        .send()
        .await
        .map_err(|err| format!("下载失败: {err}"))?;

    let status = response.status();
    if !status.is_success() {
        return Err(format!("下载 {url} 返回 {status}"));
    }

    let bytes = response
        .bytes()
        .await
        .map_err(|err| format!("读取下载内容失败: {err}"))?;

    std::fs::write(target, &bytes).map_err(|err| format!("写入 {} 失败: {err}", target.display()))?;

    Ok(format!("已保存到 {}({})", target.display(), file.human_size()))
}

pub(super) async fn course_files(arguments: serde_json::Value) -> Result<String, String> {
    let course_id = arguments["course_id"]
        .as_str()
        .ok_or_else(|| "course_files 缺少 course_id 参数".to_string())?;
    let action = arguments["action"]
        .as_str()
        .unwrap_or("list")
        .trim()
        .to_lowercase();
    let target = arguments["file"]
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let requested_path = arguments["save_path"]
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty());

    let token = std::env::var("CANVAS_API_TOKEN")
        .map_err(|_| "canvas apitoken not found ,please check your dotenv config".to_string())?;
    let client = reqwest::Client::new();
    let files = fetch_course_files(&client, &token, course_id).await?;

    if action == "list" {
        if files.is_empty() {
            return Ok(format!("课程 {course_id} 的文件列表是空的。"));
        }

        let mut text = format!("课程 {course_id} 共 {} 个文件：\n", files.len());
        for file in &files {
            let kind = file.content_type.as_deref().unwrap_or("未知类型");
            text.push_str(&format!(
                "  [{}] {}  {}  {}\n",
                file.id,
                file.display_name,
                kind,
                file.human_size()
            ));
        }
        text.push_str(&format!(
            "\n网页地址：https://oc.sjtu.edu.cn/courses/{course_id}/files"
        ));
        if files.len() == FILES_MAX_PAGES * 100 {
            text.push_str("\n（列表可能有截断，只取了前 1000 个文件）");
        }
        return Ok(clip(&text));
    }

    if action != "download" && action != "open" {
        return Err(format!("未知的 action「{action}」，只支持 list / download / open"));
    }

    let target = target.ok_or_else(|| {
        format!("action={action} 时必须给出 file（文件名或文件 id），可先用 action=list 查看")
    })?;
    let file = pick_file(&files, target)?;
    let path = save_target(file, requested_path);

    let mut report = download_file(&client, &token, file, &path).await?;
    report.push_str(&format!("\n文件：{}（{}）", file.display_name, file.human_size()));

    if action == "open" {
        open_with_system(&path)?;
        report.push_str("\n已用系统默认程序打开。");
    }

    Ok(report)
}
