
use mail_parser::MimeHeaders;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

const MAX_OUTPUT_CHARS: usize = 16000;
const PDF_SHORT_PAGES: usize = 12;
const PDF_PAGE_HEAD_CHARS: usize = 60;

pub async fn call(name: &str, arguments: serde_json::Value) -> Result<String, String> {
    match name {
        "bash" => run_bash(arguments).await,
        "read" => run_read(arguments).await,
        "write" => run_write(arguments).await,
        "edit" => run_edit(arguments).await,
        "get_courses"=>get_courses(arguments).await,
        "get_exam"=>get_exam(arguments).await,
        "watch_shuiyuan"=>watch_shuiyuan(arguments).await,
        "pdf_analysis"=>pdf_analysis(arguments).await,
        "course_files"=>course_files(arguments).await,
        "watch_eduinfo"=>watch_eduinfo(arguments).await,
        "mail_fetch"=>mail_fetch(arguments).await,
        "open_usual_website"=>open_usual_website(arguments).await,
        _ => Err("tool not found ,check tool name".to_string()),
    }
}

fn clip(text: &str) -> String {
    if text.len() < MAX_OUTPUT_CHARS {
        text.to_string()
    } else {
        let end = text.floor_char_boundary(MAX_OUTPUT_CHARS);
        format!("{}\n<output clipped>", &text[..end])
    }
}

async fn run_bash(arguments: serde_json::Value) -> Result<String, String> {
    let command = arguments["command"]
        .as_str()
        .ok_or_else(|| "bash 缺少 command 参数".to_string())?;

    let output = std::process::Command::new("bash")
        .arg("-c")
        .arg(command)
        .output()
        .map_err(|err| err.to_string())?;

    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    if !output.status.success() {
        text.push_str(&format!(
            "\n[退出码: {}]",
            output.status.code().unwrap_or(-1)
        ));
    }

    Ok(clip(&text))
}

async fn run_read(arguments: serde_json::Value) -> Result<String, String> {
    let path = arguments["path"]
        .as_str()
        .ok_or_else(|| "read 缺少 path 参数".to_string())?;
    let offset = arguments["offset"].as_u64().unwrap_or(1).max(1) as usize;
    let limit = arguments["limit"].as_u64().map(|value| value as usize);

    let meta = std::fs::metadata(path).map_err(|err| err.to_string())?;
    if meta.is_dir() {
        let mut names = Vec::new();
        for entry in std::fs::read_dir(path).map_err(|err| err.to_string())? {
            let entry = entry.map_err(|err| err.to_string())?;
            let mut name = entry.file_name().to_string_lossy().into_owned();
            if entry.file_type().map_err(|err| err.to_string())?.is_dir() {
                name.push('/');
            }
            names.push(name);
        }
        names.sort();
        return Ok(clip(&names.join("\n")));
    }

    let content = std::fs::read_to_string(path).map_err(|err| err.to_string())?;
    let lines: Vec<&str> = content.lines().collect();
    let start = offset - 1;
    if start >= lines.len() {
        return Ok(format!(
            "文件共 {} 行，offset {offset} 已超出范围",
            lines.len()
        ));
    }
    let end = match limit {
        Some(limit) => (start + limit).min(lines.len()),
        None => lines.len(),
    };

    let mut text = String::new();
    for (index, line) in lines[start..end].iter().enumerate() {
        text.push_str(&format!("{}: {line}\n", start + index + 1));
    }
    Ok(clip(&text))
}

async fn run_write(arguments: serde_json::Value) -> Result<String, String> {
    let path = arguments["path"]
        .as_str()
        .ok_or_else(|| "write 缺少 path 参数".to_string())?;
    let content = arguments["content"]
        .as_str()
        .ok_or_else(|| "write 缺少 content 参数".to_string())?;

    if let Some(parent) = std::path::Path::new(path).parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|err| err.to_string())?;
        }
    }

    std::fs::write(path, content).map_err(|err| err.to_string())?;
    Ok(format!("已写入 {path}，{} 字节", content.len()))
}

async fn run_edit(arguments: serde_json::Value) -> Result<String, String> {
    let path = arguments["path"]
        .as_str()
        .ok_or_else(|| "edit 缺少 path 参数".to_string())?;
    let old_context = arguments["old_context"]
        .as_str()
        .ok_or_else(|| "edit 缺少 old_context 参数".to_string())?;
    let new_context = arguments["new_context"]
        .as_str()
        .ok_or_else(|| "edit 缺少 new_context 参数".to_string())?;

    let content = std::fs::read_to_string(path).map_err(|err| err.to_string())?;
    match content.matches(old_context).count() {
        0 => return Err(format!("old_context 在 {path} 中未找到")),
        1 => {}
        count => return Err(format!("old_context 在 {path} 中匹配到 {count} 处，需唯一")),
    }

    let updated = content.replacen(old_context, new_context, 1);
    std::fs::write(path, updated).map_err(|err| err.to_string())?;
    Ok(format!("已替换 {path} 中的 1 处内容"))
}

#[derive(serde::Deserialize)]
    struct Course {
        id: u64,
        name: String,
        #[serde(default)]
        term: Option<Term>,
    }

    #[derive(serde::Deserialize)]
    struct Term {
        #[serde(default)]
        name: Option<String>,
    }

async fn get_courses(_: serde_json::Value)->Result<String,String>{
    let token_result=std::env::var("CANVAS_API_TOKEN");
    let token:String;
    match token_result{
        Ok(apitoken)=>token=apitoken,
        Err(_)=>token="not found".to_string()
    }
    if token=="not found".to_string(){
        return Err("canvas apitoken not found ,please check your dotenv config".to_string())
    }
    let base_url="https://oc.sjtu.edu.cn".to_string();
    let response = reqwest::Client::new()
            .get(format!("{}/api/v1/courses", base_url.trim_end_matches('/')))
            .bearer_auth(&token)
            .query(&[
                ("enrollment_state", "active"),
                ("include[]", "term"),
                ("per_page", "100"),
            ])
            .send()
            .await
            .map_err(|err| format!("请求 Canvas 失败: {err}"))?;
    let status = response.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        return Err("认证失败 (401)：令牌无效或已过期，请重新生成".to_string());
    }
    if !status.is_success() {
        return Err(format!("Canvas 返回 {status}"));
    }

    let courses: Vec<Course> = response
        .json()
        .await
        .map_err(|err| format!("解析响应失败（可能被重定向到登录页）: {err}"))?;

    if courses.is_empty() {
        return Ok("没有进行中的课程。".to_string());
    }

    let mut text = format!("共 {} 门课程：\n", courses.len());
    for course in &courses {
        text.push_str(&format!("  [{:>7}]  {}\n", course.id, course.name));
        if let Some(term) = course.term.as_ref().and_then(|t| t.name.as_deref()) {
            text.push_str(&format!("            {term}\n"));
        }
    }

    Ok(clip(&text))
    
}


#[derive(serde::Deserialize)]
    struct Assignment {
        id: u64,
        name: String,
        #[serde(default)]
        due_at: Option<String>,
        #[serde(default)]
        points_possible: Option<f64>,
        #[serde(default)]
        html_url: Option<String>,
    }

#[derive(serde::Deserialize)]
struct AssignmentDetail {
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    submission_types: Vec<String>,
    #[serde(default)]
    allowed_attempts: Option<i64>,
    #[serde(default)]
    unlock_at: Option<String>,
    #[serde(default)]
    lock_at: Option<String>,
}


async fn get_exam(arguments: serde_json::Value)->Result<String,String>{
    let course_id=arguments.get("course_id").unwrap().as_str();
    let mut base_url="https://oc.sjtu.edu.cn/courses/".to_string();
    match course_id{
        Some(id)=>base_url.push_str(id),
        None=>()
    }
    if base_url=="https://oc.sjtu.edu.cn/courses/".to_string(){
        return Err("course not found".to_string())
    }
    let id = course_id.ok_or_else(|| "course not found".to_string())?;

    let token = std::env::var("CANVAS_API_TOKEN")
        .map_err(|_| "canvas apitoken not found ,please check your dotenv config".to_string())?;

    let api_url = format!("https://oc.sjtu.edu.cn/api/v1/courses/{id}/assignments");

    let client = reqwest::Client::new();

    let response = client
        .get(&api_url)
        .bearer_auth(&token)
        .query(&[("per_page", "100")])
        .send()
        .await
        .map_err(|err| format!("请求 Canvas 失败: {err}"))?;

    let status = response.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        return Err("认证失败 (401)：令牌无效或已过期".to_string());
    }
    if status == reqwest::StatusCode::NOT_FOUND {
        return Err(format!("课程 {id} 不存在，或该课程未启用「作业」页面"));
    }
    if !status.is_success() {
        return Err(format!("Canvas 返回 {status}"));
    }

    let assignments: Vec<Assignment> = response
        .json()
        .await
        .map_err(|err| format!("解析响应失败: {err}"))?;

    if assignments.is_empty() {
        return Ok(format!("课程 {id} 还没有公布作业。"));
    }

    let mut text = format!("课程 {id} 的作业（共 {} 项）：\n", assignments.len());
    for assignment in &assignments {
        text.push_str(&format!("\n[{}] {}\n", assignment.id, assignment.name));
        text.push_str(&format!(
            "截止：{}\n",
            assignment.due_at.as_deref().unwrap_or("未设置")
        ));
        if let Some(points) = assignment.points_possible {
            text.push_str(&format!("分值：{points}\n"));
        }
        if let Some(url) = &assignment.html_url {
            text.push_str(&format!("链接：https://oc.sjtu.edu.cn{url}\n"));
        }

        let detail_url = format!(
            "https://oc.sjtu.edu.cn/api/v1/courses/{id}/assignments/{}",
            assignment.id
        );
        let detail = client
            .get(&detail_url)
            .bearer_auth(&token)
            .send()
            .await
            .and_then(|response| response.error_for_status());

        match detail {
            Ok(response) => match response.json::<AssignmentDetail>().await {
                Ok(detail) => {
                    if let Some(unlock_at) = &detail.unlock_at {
                        text.push_str(&format!("开始：{unlock_at}\n"));
                    }
                    if let Some(lock_at) = &detail.lock_at {
                        text.push_str(&format!("锁定：{lock_at}\n"));
                    }
                    if let Some(attempts) = detail.allowed_attempts {
                        let times = if attempts < 0 {
                            "不限".to_string()
                        } else {
                            attempts.to_string()
                        };
                        text.push_str(&format!("允许尝试：{times}\n"));
                    }
                    if !detail.submission_types.is_empty() {
                        text.push_str(&format!(
                            "提交方式：{}\n",
                            detail.submission_types.join("、")
                        ));
                    }
                    if let Some(description) = detail.description.as_deref() {
                        let description = description.trim();
                        if !description.is_empty() {
                            text.push_str(&format!("说明：{description}\n"));
                        }
                    }
                }
                Err(err) => text.push_str(&format!("详情解析失败：{err}\n")),
            },
            Err(err) => text.push_str(&format!("详情获取失败：{err}\n")),
        }
    }

    Ok(clip(&text))
}


async fn watch_shuiyuan(_: serde_json::Value)->Result<String,String>{
    let base_url="https://shuiyuan.sjtu.edu.cn";
    let _= webbrowser::open(base_url);
    Ok("open browser sucessful".to_string())
}
async fn watch_eduinfo(_: serde_json::Value)->Result<String,String>{
    let base_url="https://i.sjtu.edu.cn/xtgl/login_slogin.html";
    let _= webbrowser::open(base_url);
    Ok("open browser sucessful".to_string())
}
async fn open_usual_website(arguments: serde_json::Value)->Result<String,String>{
    let base_url = arguments["url"].as_str().unwrap();
    match base_url{
        "none"=>Err("不可知用户指定的网站具体url，请求提供".to_string()),
        _=>{let _= webbrowser::open(base_url);Ok("open website sucessful".to_string())}
    }   
}















fn pdf_cache() -> &'static Mutex<HashMap<String, Arc<Vec<String>>>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Arc<Vec<String>>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

async fn load_pdf_pages(path: &str) -> Result<Arc<Vec<String>>, String> {
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

async fn pdf_analysis(arguments: serde_json::Value) -> Result<String, String> {
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













const FILES_MAX_PAGES: usize = 10;

#[derive(serde::Deserialize)]
struct CanvasFile {
    id: u64,
    display_name: String,
    #[serde(default)]
    filename: Option<String>,
    #[serde(default, rename = "content-type")]
    content_type: Option<String>,
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
    fn name(&self) -> &str {
        if self.display_name.trim().is_empty() {
            self.filename.as_deref().unwrap_or("(未命名)")
        } else {
            &self.display_name
        }
    }

    /// 落盘用的文件名。Canvas 对非 ASCII 名字会做百分号编码
    /// （如 "01.+%E8%AF%BE%E7%A8%8B%E7%AE%80%E4%BB%8B.pdf"），直接当路径会写出一串乱码。
    fn local_name(&self) -> String {
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

async fn download_file(
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

async fn course_files(arguments: serde_json::Value) -> Result<String, String> {
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


























const MAIL_TIMEOUT_SECS: u64 = 30;
const MAIL_MAX_LIMIT: usize = 50;


struct MailConfig {
    host: String,
    port: u16,
    user: String,
    pass: String,
}

impl MailConfig {
    fn from_env() -> Result<Self, String> {
        let user = std::env::var("EMAIL_USER_ACCOUNT")
            .map_err(|_| "缺少 EMAIL_USER_ACCOUNT".to_string())?;
        let pass = std::env::var("EMAIL_USER_PASSWORD")
            .map_err(|_| "缺少 EMAIL_USER_PASSWORD".to_string())?;
        let host =
            std::env::var("IMAP_HOST").map_err(|_| "缺少 IMAP_HOST（请在 .env 里配置）".to_string())?;
        let port = std::env::var("IMAP_PORT")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(993);
        Ok(Self {
            host,
            port,
            user,
            pass,
        })
    }
}

type MailSession = imap::Session<native_tls::TlsStream<std::net::TcpStream>>;

fn mail_connect(config: &MailConfig) -> Result<MailSession, String> {
    use std::net::ToSocketAddrs;

    let timeout = std::time::Duration::from_secs(MAIL_TIMEOUT_SECS);
    let address = (config.host.as_str(), config.port)
        .to_socket_addrs()
        .map_err(|err| format!("解析 {} 失败: {err}", config.host))?
        .next()
        .ok_or_else(|| format!("{}:{} 没有可用地址", config.host, config.port))?;

    let stream = std::net::TcpStream::connect_timeout(&address, timeout)
        .map_err(|err| format!("连接 {address} 失败: {err}"))?;
    // 不设超时的话，服务器卡住会把整个 ReAct 轮次拖死
    stream
        .set_read_timeout(Some(timeout))
        .map_err(|err| err.to_string())?;
    stream
        .set_write_timeout(Some(timeout))
        .map_err(|err| err.to_string())?;

    let tls = native_tls::TlsConnector::builder()
        .build()
        .map_err(|err| format!("初始化 TLS 失败: {err}"))?;
    let tls_stream = tls
        .connect(&config.host, stream)
        .map_err(|err| format!("TLS 握手失败: {err}"))?;

    imap::Client::new(tls_stream)
        .login(&config.user, &config.pass)
        .map_err(|(err, _)| format!("登录失败（确认用的是客户端授权码而不是登录密码）: {err}"))
}

fn format_addr(addr: &mail_parser::Addr<'_>) -> String {
    match (addr.name.as_deref(), addr.address.as_deref()) {
        // 裸地址时 mail-parser 会把地址同时填进 name，避免打成 "a@b <a@b>"
        (Some(name), Some(mail)) if name != mail => format!("{name} <{mail}>"),
        (_, Some(mail)) => mail.to_string(),
        (Some(name), None) => name.to_string(),
        (None, None) => "(未知发件人)".to_string(),
    }
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// IMAP 的 UID 集合是字符串形式，形如 "12,34,56"。
fn uid_set(uids: &[u32]) -> String {
    uids.iter()
        .map(|uid| uid.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

fn mail_list(
    session: &mut MailSession,
    folder: &str,
    limit: usize,
    unread_only: bool,
) -> Result<String, String> {
    // EXAMINE 而不是 SELECT：只读打开，不会改动任何标记
    session.examine(folder).map_err(|err| format!("打开 {folder} 失败: {err}"))?;

    let query = if unread_only { "UNSEEN" } else { "ALL" };
    let mut uids: Vec<u32> = session
        .uid_search(query)
        .map_err(|err| format!("搜索邮件失败: {err}"))?
        .into_iter()
        .collect();
    uids.sort_unstable();

    let matched = uids.len();
    let targets: Vec<u32> = uids.into_iter().rev().take(limit).collect();
    if targets.is_empty() {
        return Ok(format!("{folder} 里没有符合条件的邮件。"));
    }

    // BODY.PEEK 而不是 BODY：取 header 不会把邮件标成已读
    let uid_set = uid_set(&targets);
    let messages = session
        .uid_fetch(&uid_set, "(FLAGS RFC822.SIZE BODY.PEEK[HEADER])")
        .map_err(|err| format!("读取邮件列表失败: {err}"))?;

    let parser = mail_parser::MessageParser::default();
    let mut text = format!(
        "{folder}：共匹配 {matched} 封，下面是最新的 {} 封\n",
        targets.len()
    );

    for message in messages.iter() {
        let uid = message.uid.unwrap_or(0);
        let seen = message
            .flags()
            .iter()
            .any(|flag| matches!(flag, imap::types::Flag::Seen));
        let size = message.size.unwrap_or(0);
        let parsed = message.header().and_then(|raw| parser.parse(raw));

        let (from, subject, date) = match &parsed {
            Some(parsed) => (
                parsed
                    .from()
                    .and_then(|list| list.first())
                    .map(format_addr)
                    .unwrap_or_default(),
                parsed.subject().unwrap_or("(无主题)").to_string(),
                parsed
                    .date()
                    .map(|date| date.to_rfc3339())
                    .unwrap_or_else(|| "(无日期)".to_string()),
            ),
            None => ("(解析失败)".to_string(), "(解析失败)".to_string(), String::new()),
        };

        text.push_str(&format!(
            "  UID {uid} [{}] {} 字节\n    发件人：{}\n    主题：{}\n    时间：{}\n",
            if seen { "已读" } else { "未读" },
            size,
            one_line(&from),
            one_line(&subject),
            date
        ));
    }

    text.push_str("\n要读某一封的正文，用 action=read 和对应的 uid。");
    Ok(clip(&text))
}

fn mail_read(session: &mut MailSession, folder: &str, uid: u32) -> Result<String, String> {
    session.examine(folder).map_err(|err| format!("打开 {folder} 失败: {err}"))?;

    let messages = session
        .uid_fetch(uid.to_string(), "(FLAGS RFC822.SIZE BODY.PEEK[])")
        .map_err(|err| format!("读取邮件失败: {err}"))?;

    let raw = messages
        .iter()
        .find_map(|message| message.body())
        .ok_or_else(|| format!("{folder} 里没有 uid 为 {uid} 的邮件"))?;

    let parsed = mail_parser::MessageParser::default()
        .parse(raw)
        .ok_or_else(|| "这封邮件解析失败".to_string())?;

    let mut text = String::new();
    text.push_str(&format!(
        "主题：{}\n",
        parsed.subject().unwrap_or("(无主题)")
    ));
    text.push_str(&format!(
        "发件人：{}\n",
        parsed
            .from()
            .and_then(|list| list.first())
            .map(format_addr)
            .unwrap_or_else(|| "(未知)".to_string())
    ));
    if let Some(date) = parsed.date() {
        text.push_str(&format!("时间：{}\n", date.to_rfc3339()));
    }
    let to: Vec<String> = parsed
        .to()
        .map(|list| list.iter().map(format_addr).collect())
        .unwrap_or_default();
    if !to.is_empty() {
        text.push_str(&format!("收件人：{}\n", one_line(&to.join(", "))));
    }

    let attachments: Vec<_> = parsed.attachments().collect();
    if !attachments.is_empty() {
        text.push_str("附件：\n");
        for attachment in attachments {
            text.push_str(&format!(
                "  - {}（{} 字节）\n",
                attachment.attachment_name().unwrap_or("(未命名)"),
                attachment.len()
            ));
        }
    }

    // body_text 在只有 HTML 的邮件里会自动转成纯文本，不用自己剥标签
    let body = parsed
        .body_text(0)
        .map(|body| body.trim().to_string())
        .filter(|body| !body.is_empty())
        .unwrap_or_else(|| "(没有可显示的正文)".to_string());
    text.push_str("\n———— 正文 ————\n");
    text.push_str(&body);

    Ok(clip(&text))
}

fn mail_folders(session: &mut MailSession) -> Result<String, String> {
    let folders = session
        .list(None, Some("*"))
        .map_err(|err| format!("列出文件夹失败: {err}"))?;

    let mut text = String::from("邮箱文件夹：\n");
    for folder in folders.iter() {
        text.push_str(&format!("  {}\n", folder.name()));
    }
    Ok(text)
}

fn run_mail(
    action: String,
    folder: String,
    limit: usize,
    uid: Option<u32>,
    unread_only: bool,
) -> Result<String, String> {
    let config = MailConfig::from_env()?;
    let mut session = mail_connect(&config)?;

    let result = match action.as_str() {
        "list" => mail_list(&mut session, &folder, limit, unread_only),
        "read" => {
            let uid = uid.ok_or_else(|| "action=read 时必须给出 uid，可先用 action=list 查看".to_string())?;
            mail_read(&mut session, &folder, uid)
        }
        "folders" => mail_folders(&mut session),
        other => Err(format!("未知的 action「{other}」，只支持 list / read / folders")),
    };

    let _ = session.logout();
    result
}

async fn mail_fetch(arguments: serde_json::Value) -> Result<String, String> {
    let action = arguments["action"]
        .as_str()
        .unwrap_or("list")
        .trim()
        .to_lowercase();
    let folder = arguments["folder"]
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("INBOX")
        .to_string();
    let limit = match arguments["limit"].as_u64() {
        Some(value) => (value as usize).clamp(1, MAIL_MAX_LIMIT),
        None => 10,
    };
    let uid = match &arguments["uid"] {
        serde_json::Value::Number(number) => number.as_u64().map(|value| value as u32),
        serde_json::Value::String(text) => text.trim().parse().ok(),
        _ => None,
    };
    let unread_only = arguments["unread_only"].as_bool().unwrap_or(false);

    tokio::task::spawn_blocking(move || run_mail(action, folder, limit, uid, unread_only))
        .await
        .map_err(|err| format!("邮件任务失败: {err}"))?
}