use std::collections::HashSet;
use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt;

use super::canvas::{CANVAS_BASE, next_link, same_origin};
use super::clip;

#[derive(serde::Deserialize)]
pub(super) struct CanvasFile {
    pub(super) id: u64,
    pub(super) display_name: String,
    #[serde(default)]
    filename: Option<String>,
    #[serde(default, rename = "content-type")]
    pub(super) content_type: Option<String>,
    #[serde(default)]
    size: Option<u64>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    pub(super) updated_at: Option<String>,
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

    pub(super) fn human_size(&self) -> String {
        match self.size {
            Some(size) if size >= 1024 * 1024 => format!("{:.1} MB", size as f64 / 1048576.0),
            Some(size) if size >= 1024 => format!("{:.0} KB", size as f64 / 1024.0),
            Some(size) => format!("{size} B"),
            None => "大小未知".to_string(),
        }
    }

    fn bulk_name(&self) -> String {
        let name = self.local_name();
        let prefix = format!("{}-", self.id);
        let available = 255 - prefix.len();
        if name.len() <= available {
            return format!("{prefix}{name}");
        }
        let suffix = name
            .rfind('.')
            .map(|at| &name[at..])
            .filter(|s| s.len() <= 32)
            .unwrap_or("");
        let end = name.floor_char_boundary(available - suffix.len());
        format!("{prefix}{}{suffix}", &name[..end])
    }
}

fn course_segment(course_id: &str) -> Result<String, String> {
    if matches!(course_id, "" | "." | "..") || course_id.chars().any(char::is_control) {
        return Err("课程 id 无效，可用 get_courses 查看".into());
    }
    let mut url = reqwest::Url::parse(CANVAS_BASE).expect("Canvas HTTPS origin");
    url.path_segments_mut().unwrap().push(course_id);
    Ok(url.path().trim_start_matches('/').to_string())
}

pub(super) async fn fetch_course_files(
    client: &reqwest::Client,
    token: &str,
    course_id: &str,
) -> Result<Vec<CanvasFile>, String> {
    let course_id = course_segment(course_id)?;
    let mut url = format!("{CANVAS_BASE}/api/v1/courses/{course_id}/files?per_page=100&page=1");
    let mut files = Vec::new();
    let mut pages = HashSet::new();
    let mut ids = HashSet::new();

    loop {
        if !same_origin(&url, CANVAS_BASE) || !pages.insert(url.clone()) {
            return Err("Canvas 文件分页地址异常，未能取得完整列表".into());
        }
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
            return Err(format!(
                "课程 {course_id} 不存在，或该课程未启用「文件」页面"
            ));
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
        files.extend(batch.into_iter().filter(|file| ids.insert(file.id)));

        match next {
            Some(next_url) => url = next_url,
            None => break,
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
        0 => Err(format!(
            "没有匹配「{target}」的文件，可先用 action=list 查看"
        )),
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

fn downloads_dir() -> PathBuf {
    std::env::var("HOME")
        .map(|home| PathBuf::from(home).join("Downloads"))
        .unwrap_or_else(|_| PathBuf::from("."))
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

async fn download_to_temp(
    client: &reqwest::Client,
    token: &str,
    file: &CanvasFile,
    target: &Path,
) -> Result<tempfile::NamedTempFile, String> {
    let parent = target
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent).map_err(|err| err.to_string())?;
    let url = file
        .url
        .as_deref()
        .ok_or_else(|| format!("文件 {} 没有提供下载地址", file.display_name))?;

    let url = reqwest::Url::parse(url).map_err(|_| "Canvas 返回了无效的下载地址")?;
    if url.scheme() != "https" {
        return Err("Canvas 文件下载需要 HTTPS".into());
    }
    let mut request = client.get(url.clone());
    if same_origin(url.as_str(), CANVAS_BASE) {
        request = request.bearer_auth(token);
    }
    let mut response = request
        .send()
        .await
        .map_err(|err| format!("下载失败：{}", err.without_url()))?;

    let status = response.status();
    if !status.is_success() {
        return Err(format!("下载失败，服务器返回 {status}"));
    }

    let temp = tempfile::NamedTempFile::new_in(parent).map_err(|err| err.to_string())?;
    let mut output = tokio::fs::File::from_std(temp.reopen().map_err(|err| err.to_string())?);
    let mut length = 0u64;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|err| err.without_url().to_string())?
    {
        output
            .write_all(&chunk)
            .await
            .map_err(|err| err.to_string())?;
        length += chunk.len() as u64;
    }
    output.flush().await.map_err(|err| err.to_string())?;
    if file.size.is_some_and(|expected| expected != length) {
        return Err(format!(
            "下载字节数与 Canvas 记录不符，实际收到 {length} 字节"
        ));
    }
    Ok(temp)
}

pub(super) async fn download_file(
    client: &reqwest::Client,
    token: &str,
    file: &CanvasFile,
    target: &Path,
) -> Result<String, String> {
    download_to_temp(client, token, file, target)
        .await?
        .persist(target)
        .map_err(|err| format!("写入 {} 失败：{err}", target.display()))?;
    Ok(format!(
        "已保存到 {}({})",
        target.display(),
        file.human_size()
    ))
}

async fn download_all(
    client: &reqwest::Client,
    token: &str,
    files: &[CanvasFile],
    dir: &Path,
) -> Result<String, String> {
    std::fs::create_dir_all(dir).map_err(|err| format!("无法创建目录 {}：{err}", dir.display()))?;
    let mut saved = 0;
    let mut skipped = 0;
    let mut failures = Vec::new();
    for file in files {
        let name = file.bulk_name();
        let path = dir.join(&name);
        let result = match path.symlink_metadata() {
            Ok(meta) if meta.is_file() && file.size.is_none_or(|size| size == meta.len()) => {
                skipped += 1;
                continue;
            }
            Ok(_) => Err("同名路径已存在，请移走该项后重试".into()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                match download_to_temp(client, token, file, &path).await {
                    Ok(temp) => temp
                        .persist_noclobber(&path)
                        .map(|_| ())
                        .map_err(|err| err.error.to_string()),
                    Err(err) => Err(err),
                }
            }
            Err(err) => Err(err.to_string()),
        };
        match result {
            Ok(()) => saved += 1,
            Err(err) => failures.push(format!("{name}：{err}")),
        }
    }
    let report = format!(
        "{} 个课程文件，已下载 {saved} 个，已有 {skipped} 个，失败 {} 个。\n目录：{}",
        files.len(),
        failures.len(),
        dir.display()
    );
    if failures.is_empty() {
        Ok(report)
    } else {
        Err(clip(&format!("{report}\n{}", failures.join("\n"))))
    }
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
        return Ok(clip(&text));
    }

    if action == "download_all" {
        let segment = course_segment(course_id)?;
        let dir = requested_path
            .map(PathBuf::from)
            .unwrap_or_else(|| downloads_dir().join(format!("Canvas-{segment}")));
        return download_all(&client, &token, &files, &dir).await;
    }
    if action != "download" && action != "open" {
        return Err(format!(
            "未知的 action「{action}」，只支持 list / download / download_all / open"
        ));
    }

    let target = target.ok_or_else(|| {
        format!("action={action} 时必须给出 file（文件名或文件 id），可先用 action=list 查看")
    })?;
    let file = pick_file(&files, target)?;
    let path = requested_path
        .map(PathBuf::from)
        .unwrap_or_else(|| downloads_dir().join(file.local_name()));

    let mut report = download_file(&client, &token, file, &path).await?;
    report.push_str(&format!(
        "\n文件：{}（{}）",
        file.display_name,
        file.human_size()
    ));

    if action == "open" {
        open_with_system(&path)?;
        report.push_str("\n已用系统默认程序打开。");
    }

    Ok(report)
}
