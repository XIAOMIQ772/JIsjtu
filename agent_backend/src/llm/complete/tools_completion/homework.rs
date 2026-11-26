use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;

use super::canvas::{CANVAS_BASE, same_origin};
use super::course_files::{CanvasFile, download_file};
use super::office::document_pages;
use super::pdf_analysis::load_pdf_pages;
use super::text::html_to_text;
use super::MAX_OUTPUT_CHARS;

mod artifact;
mod questions;
mod requirements;
use questions::QuestionStyle;
use questions::{homework_page, part_number, render_questions, split_question_sources, split_questions};
use requirements::RubricCriterion;

static HW_FILE_LINK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"/(?:courses/\d+/)?files/(\d+)").unwrap());

fn text_to_html(text: &str) -> String {
    if text.trim_start().starts_with('<') {
        return text.to_string();
    }
    let escape = |line: &str| {
        let indent = line.len() - line.trim_start().len();
        let body = line
            .trim_start()
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;");
        format!("{}{body}", "&nbsp;".repeat(indent))
    };
    text.replace("\r\n", "\n")
        .split("\n\n")
        .filter(|paragraph| !paragraph.trim().is_empty())
        .map(|paragraph| {
            let lines: Vec<String> = paragraph.lines().map(escape).collect();
            format!("<p>{}</p>", lines.join("<br>\n"))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[derive(serde::Deserialize)]
struct HomeworkAssignment {
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    due_at: Option<String>,
    #[serde(default)]
    points_possible: Option<f64>,
    #[serde(default)]
    submission_types: Option<Vec<String>>,
    #[serde(default)]
    allowed_extensions: Option<Vec<String>>,
    #[serde(default)]
    locked_for_user: bool,
    #[serde(default)]
    lock_explanation: Option<String>,
    #[serde(default)]
    html_url: Option<String>,
    #[serde(default)]
    rubric: Option<Vec<RubricCriterion>>,
    #[serde(default)]
    use_rubric_for_grading: Option<bool>,
}

impl HomeworkAssignment {
    fn summary(&self) -> String {
        let mut text = format!("作业：{}\n截止：{}\n", self.name, self.due_at.as_deref().unwrap_or("未设置"));
        if let Some(points) = self.points_possible {
            text.push_str(&format!("分值：{points}\n"));
        }
        if let Some(types) = self.submission_types.as_ref().filter(|types| !types.is_empty()) {
            text.push_str(&format!("提交方式：{}\n", types.join("、")));
        }
        if let Some(extensions) = self.allowed_extensions.as_ref().filter(|ext| !ext.is_empty()) {
            text.push_str(&format!("允许的文件类型：{}\n", extensions.join("、")));
        }
        if let Some(url) = &self.html_url {
            text.push_str(&format!("链接：{url}\n"));
        }
        text
    }
}

fn canvas_token() -> Result<String, String> {
    std::env::var("CANVAS_API_TOKEN")
        .map_err(|_| "canvas apitoken not found ,please check your dotenv config".to_string())
}

async fn canvas_failure(response: reqwest::Response, action: &str) -> String {
    let status = response.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        return "认证失败 (401)：令牌无效或已过期".to_string();
    }
    let body = response.text().await.unwrap_or_default();
    let body: String = body.chars().take(500).collect();
    format!("{action}失败，Canvas 返回 {status}：{body}")
}

async fn fetch_homework_assignment(
    client: &reqwest::Client,
    token: &str,
    course_id: &str,
    assignment_id: &str,
) -> Result<HomeworkAssignment, String> {
    let response = client
        .get(format!("{CANVAS_BASE}/api/v1/courses/{course_id}/assignments/{assignment_id}"))
        .bearer_auth(token)
        .send()
        .await
        .map_err(|err| format!("请求 Canvas 失败: {err}"))?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Err(format!("课程 {course_id} 中没有 id 为 {assignment_id} 的作业，可先用 get_exam 查看"));
    }
    if !response.status().is_success() {
        return Err(canvas_failure(response, "读取作业").await);
    }
    response
        .json()
        .await
        .map_err(|err| format!("解析作业详情失败: {err}"))
}

fn homework_arg<'a>(arguments: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    arguments[key]
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

/// 下载作业说明里链接的附件，返回 (本地路径, 是否 PDF) 和下载失败的说明。
async fn download_homework_attachments(
    client: &reqwest::Client,
    token: &str,
    html: &str,
) -> (Vec<(PathBuf, bool)>, Vec<String>) {
    let mut ids: Vec<&str> = Vec::new();
    for caps in HW_FILE_LINK.captures_iter(html) {
        let id = caps.get(1).unwrap().as_str();
        if !ids.contains(&id) {
            ids.push(id);
        }
    }

    let dir = std::env::temp_dir().join("jisjtu-homework");
    let mut saved = Vec::new();
    let mut failures = Vec::new();
    for id in ids {
        let file = match client
            .get(format!("{CANVAS_BASE}/api/v1/files/{id}"))
            .bearer_auth(token)
            .send()
            .await
            .and_then(|response| response.error_for_status())
        {
            Ok(response) => match response.json::<CanvasFile>().await {
                Ok(file) => file,
                Err(err) => {
                    failures.push(format!("附件 {id} 信息解析失败：{err}"));
                    continue;
                }
            },
            Err(err) => {
                failures.push(format!("附件 {id} 获取失败：{err}"));
                continue;
            }
        };
        // Different Canvas attachments may intentionally have identical display names.
        let target = dir.join(format!("file-{id}")).join(file.local_name());
        match download_file(client, token, &file, &target).await {
            Ok(_) => {
                let is_pdf = file.content_type.as_deref().is_some_and(|kind| kind.contains("pdf"))
                    || file.local_name().to_lowercase().ends_with(".pdf");
                saved.push((target, is_pdf));
            }
            Err(err) => failures.push(format!("附件 {} 下载失败：{err}", file.name())),
        }
    }
    (saved, failures)
}

async fn homework_questions(arguments: &serde_json::Value) -> Result<String, String> {
    let only = homework_arg(arguments, "question");
    let part = part_number(arguments)?;

    if let Some(text) = arguments["text"].as_str().filter(|t| !t.trim().is_empty()) {
        let parsed = split_questions(&[text.to_string()]);
        return render_questions("来源：用户提供的作业文字\n", &parsed, only, part);
    }

    if let Some(path) = homework_arg(arguments, "path") {
        let is_pdf = path.to_lowercase().ends_with(".pdf")
            || std::fs::read(path).map(|bytes| bytes.starts_with(b"%PDF")).unwrap_or(false);
        let pages: Vec<String> = if is_pdf {
            load_pdf_pages(path).await?.as_ref().clone()
        } else if let Some(pages) = document_pages(Path::new(path)) {
            pages?
        } else {
            vec![tokio::fs::read_to_string(path)
                .await
                .map_err(|err| format!("读取 {path} 失败: {err}"))?]
        };
        let header = format!("来源：{path}（共 {} 页）\n", pages.len());
        return render_questions(&header, &split_questions(&pages), only, part);
    }

    let (Some(course_id), Some(assignment_id)) = (
        homework_arg(arguments, "course_id"),
        homework_arg(arguments, "assignment_id"),
    ) else {
        return Err("请提供题目来源：path（本地 PDF、Word、PowerPoint 或文本文件）、text（作业文字）或 course_id + assignment_id（Canvas 作业）".to_string());
    };

    let token = canvas_token()?;
    let client = reqwest::Client::new();
    let assignment = fetch_homework_assignment(&client, &token, course_id, assignment_id).await?;
    let html = assignment.description.as_deref().unwrap_or_default();
    let description = html_to_text(html);
    let (attachments, failures) =
        download_homework_attachments(&client, &token, html).await;

    let mut header = assignment.summary();
    for failure in &failures {
        header.push_str(&format!("{failure}\n"));
    }

    let mut sources = Vec::new();
    for (path, is_pdf) in &attachments {
        let loaded: Result<Vec<String>, String> = if *is_pdf {
            load_pdf_pages(&path.to_string_lossy()).await.map(|pages| pages.as_ref().clone())
        } else if let Some(result) = document_pages(path) {
            result
        } else if path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("txt") || ext.eq_ignore_ascii_case("md")) {
            tokio::fs::read_to_string(path).await.map(|text| vec![text]).map_err(|err| err.to_string())
        } else {
            header.push_str(&format!("其他附件（按作业要求单独读取）：{}\n", path.display()));
            continue;
        };
        match loaded {
            Ok(pages) if pages.iter().any(|page| !page.trim().is_empty()) => sources.push((path, pages)),
            Ok(_) => header.push_str(&format!("{} 没有可提取的文字，请核对原件。\n", path.display())),
            Err(err) => header.push_str(&format!("{} 无法提取文字：{err}\n", path.display())),
        }
    }

    let web = split_questions(std::slice::from_ref(&description));
    let web_is_source = !description.is_empty() && (sources.is_empty() || web.style != QuestionStyle::Whole);
    // With attachments the webpage can contain both questions and shared submission instructions.
    let instructions = assignment.requirements(if sources.is_empty() { "" } else { &description });
    if instructions.len() <= 4_000 {
        header.push_str(&instructions);
    } else {
        header.push_str("作业说明或评分细则较长。作答前请用 action=requirements 读取全部要求，按返回的 part 续读。\n");
    }
    if sources.is_empty() {
        if description.is_empty() {
            return homework_page(&format!("{header}\n作业说明为空，也没有可读取的题面。请核对上面的附件。"), part);
        }
        header.push_str("题目来源：Canvas 作业说明\n");
        return render_questions(&header, &web, only, part);
    }
    header.push_str("题目来源与页码（按老师指定的范围作答）：\n");
    let mut documents = Vec::new();
    let mut next_page = 1;
    if web_is_source {
        header.push_str("Canvas 网页说明对应第 1 页，完整说明可用 action=requirements 读取。\n");
        documents.push(vec![description]);
        next_page += 1;
    }
    for (path, pages) in sources {
        header.push_str(&format!("{} 对应第 {}–{} 页\n", path.display(), next_page, next_page + pages.len() - 1));
        next_page += pages.len();
        documents.push(pages);
    }
    render_questions(&header, &split_question_sources(&documents), only, part)
}

async fn upload_submission_file(
    client: &reqwest::Client,
    token: &str,
    course_id: &str,
    assignment_id: &str,
    path: &Path,
    bytes: Vec<u8>,
) -> Result<u64, String> {
    #[derive(serde::Deserialize)]
    struct UploadTicket {
        upload_url: String,
        #[serde(default)]
        upload_params: serde_json::Map<String, serde_json::Value>,
    }

    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "submission".to_string());
    let content_type = mime_guess::from_path(path).first_or_octet_stream().to_string();

    let response = client
        .post(format!(
            "{CANVAS_BASE}/api/v1/courses/{course_id}/assignments/{assignment_id}/submissions/self/files"
        ))
        .bearer_auth(token)
        .form(&[
            ("name", name.clone()),
            ("size", bytes.len().to_string()),
            ("content_type", content_type.clone()),
        ])
        .send()
        .await
        .map_err(|err| format!("请求上传地址失败: {err}"))?;
    if !response.status().is_success() {
        return Err(canvas_failure(response, &format!("申请上传 {name} ")).await);
    }
    let ticket: UploadTicket = response
        .json()
        .await
        .map_err(|err| format!("解析上传地址失败: {err}"))?;

    let mut form = reqwest::multipart::Form::new();
    for (key, value) in ticket.upload_params {
        let value = match value {
            serde_json::Value::String(text) => text,
            other => other.to_string(),
        };
        form = form.text(key, value);
    }
    let part = reqwest::multipart::Part::bytes(bytes)
        .file_name(name.clone())
        .mime_str(&content_type)
        .map_err(|err| err.to_string())?;
    form = form.part("file", part);

    // 上传完成后的 3XX 必须由我们带着令牌 GET 一次才算确认，不能交给自动重定向
    let uploader = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|err| err.to_string())?;
    let response = uploader
        .post(&ticket.upload_url)
        .multipart(form)
        .send()
        .await
        .map_err(|err| format!("上传 {name} 失败: {err}"))?;

    // Canvas 的第三步有两种形式：3XX 跳到确认地址；或 201 直接给出文件，
    // 有的存储后端（inst-fs）在 201 里只给 location，也要再 GET 一次才算确认。
    let status = response.status();
    if !status.is_success() && !status.is_redirection() {
        return Err(canvas_failure(response, &format!("上传 {name} ")).await);
    }
    let header_location = response
        .headers()
        .get("location")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let body: serde_json::Value = if status.is_redirection() {
        serde_json::Value::Null
    } else {
        response.json().await.unwrap_or(serde_json::Value::Null)
    };
    if let Some(id) = body["id"].as_u64() {
        return Ok(id);
    }
    let location = body["location"]
        .as_str()
        .map(str::to_string)
        .or(header_location)
        .ok_or_else(|| format!("上传 {name} 后既没有文件 id 也没有确认地址：{body}"))?;
    let location = reqwest::Url::parse(&ticket.upload_url)
        .and_then(|base| base.join(&location))
        .map_err(|err| format!("上传 {name} 的确认地址无效（{location}）: {err}"))?;

    let confirm = client.get(location.clone());
    let confirm = if same_origin(location.as_str(), CANVAS_BASE) {
        confirm.bearer_auth(token)
    } else {
        confirm
    };
    let response = confirm
        .send()
        .await
        .map_err(|err| format!("确认上传 {name} 失败: {err}"))?;
    if !response.status().is_success() {
        return Err(canvas_failure(response, &format!("确认上传 {name} ")).await);
    }
    let file: serde_json::Value = response
        .json()
        .await
        .map_err(|err| format!("解析上传结果失败: {err}"))?;
    file["id"]
        .as_u64()
        .ok_or_else(|| format!("上传 {name} 后没有拿到文件 id：{file}"))
}

/// 工具结果里这条分隔线之后的内容由聊天循环直接展示给用户，不进入模型上下文。
pub const USER_NOTICE_MARK: &str = "\n<<<仅展示给用户>>>\n";
mod submission;

fn check_submittable(assignment: &HomeworkAssignment, kind: &str, files: &[PathBuf]) -> Result<(), String> {
    if assignment.locked_for_user {
        return Err(format!(
            "作业「{}」当前不可提交：{}",
            assignment.name,
            assignment.lock_explanation.as_deref().unwrap_or("已锁定")
        ));
    }
    let types = assignment.submission_types.clone().unwrap_or_default();
    if !types.iter().any(|allowed| allowed == kind) {
        return Err(format!(
            "作业「{}」不接受 {kind}，允许的提交方式：{}",
            assignment.name,
            if types.is_empty() { "无".to_string() } else { types.join("、") }
        ));
    }
    if let Some(extensions) = assignment.allowed_extensions.as_ref().filter(|ext| !ext.is_empty()) {
        for file in files {
            let extension = file
                .extension()
                .map(|ext| ext.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            if !extensions.iter().any(|allowed| allowed.to_lowercase() == extension) {
                return Err(format!(
                    "{} 的类型不被允许，该作业只接受：{}",
                    file.display(),
                    extensions.join("、")
                ));
            }
        }
    }
    Ok(())
}

pub(super) async fn homework(arguments: serde_json::Value) -> Result<String, String> {
    match arguments["action"].as_str().unwrap_or("questions").trim() {
        "questions" => homework_questions(&arguments).await,
        "requirements" => requirements::read(&arguments).await,
        "submit" => submission::submit_homework(&arguments).await,
        other => Err(format!("未知的 action「{other}」，只支持 questions / requirements / submit")),
    }
}

#[cfg(test)]
mod tests;
