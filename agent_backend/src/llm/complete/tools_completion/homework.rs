use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex, OnceLock};

use regex::Regex;

use super::canvas::CANVAS_BASE;
use super::course_files::{CanvasFile, download_file};
use super::office::document_pages;
use super::pdf_analysis::load_pdf_pages;
use super::text::html_to_text;
use super::MAX_OUTPUT_CHARS;

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
) -> Result<u64, String> {
    #[derive(serde::Deserialize)]
    struct UploadTicket {
        upload_url: String,
        #[serde(default)]
        upload_params: serde_json::Map<String, serde_json::Value>,
    }

    let bytes = tokio::fs::read(path)
        .await
        .map_err(|err| format!("读取 {} 失败: {err}", path.display()))?;
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "submission".to_string());
    let content_type = mime_guess::from_path(path).first_or_octet_stream().to_string();

    let response = client
        .post(format!(
            "https://oc.sjtu.edu.cn/api/v1/courses/{course_id}/assignments/{assignment_id}/submissions/self/files"
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
    let confirm = if location.host_str() == Some("oc.sjtu.edu.cn") {
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
const HOMEWORK_PREVIEW_MINUTES: u64 = 30;

/// 一份待提交的内容。预览后保存在这里，用户同意后提交的就是预览过的这一份。
struct PendingSubmission {
    course_id: String,
    assignment_id: String,
    text: Option<String>,
    files: Vec<PathBuf>,
    files_digest: u64,
    comment: Option<String>,
    created: std::time::Instant,
}

impl PendingSubmission {
    fn expired(&self) -> bool {
        self.created.elapsed().as_secs() > HOMEWORK_PREVIEW_MINUTES * 60
    }

    fn kind(&self) -> &'static str {
        if self.text.is_some() { "online_text_entry" } else { "online_upload" }
    }

    fn is_for(&self, course_id: &str, assignment_id: &str) -> bool {
        self.course_id == course_id && self.assignment_id == assignment_id
    }

    /// 给用户看的内容说明：正文或文件清单，以及附言。
    fn describe(&self) -> String {
        let mut out = String::new();
        if let Some(text) = &self.text {
            out.push_str(&format!("\n提交内容（共 {} 字）：\n{text}\n", text.chars().count()));
        }
        for file in &self.files {
            let size = std::fs::metadata(file).map(|meta| meta.len()).unwrap_or(0);
            out.push_str(&format!("\n提交文件：{}（{size} 字节）", file.display()));
        }
        if let Some(comment) = &self.comment {
            out.push_str(&format!("\n附言：{comment}\n"));
        }
        out
    }
}

fn pending_submissions() -> &'static Mutex<Vec<PendingSubmission>> {
    static PENDING: OnceLock<Mutex<Vec<PendingSubmission>>> = OnceLock::new();
    PENDING.get_or_init(|| Mutex::new(Vec::new()))
}

/// 从本次调用的参数里读出要提交的内容；text 和 files 都没给时返回 None。
fn submission_from_arguments(
    arguments: &serde_json::Value,
    course_id: &str,
    assignment_id: &str,
) -> Result<Option<PendingSubmission>, String> {
    let text = homework_arg(arguments, "text");
    let files: Vec<PathBuf> = arguments["files"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::trim).filter(|path| !path.is_empty()))
                .map(PathBuf::from)
                .collect()
        })
        .unwrap_or_default();
    match (text.is_some(), files.is_empty()) {
        (true, false) => return Err("text 和 files 只能二选一：Canvas 一次提交只能是在线文本或文件上传".to_string()),
        (false, true) => return Ok(None),
        _ => {}
    }
    for file in &files {
        if !file.is_file() {
            return Err(format!("文件 {} 不存在", file.display()));
        }
    }
    Ok(Some(PendingSubmission {
        course_id: course_id.to_string(),
        assignment_id: assignment_id.to_string(),
        text: text.map(str::to_string),
        files_digest: files_digest(&files)?,
        files,
        comment: homework_arg(arguments, "comment").map(str::to_string),
        created: std::time::Instant::now(),
    }))
}

fn files_digest(files: &[PathBuf]) -> Result<u64, String> {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for file in files {
        file.hash(&mut hasher);
        std::fs::read(file)
            .map_err(|err| format!("读取 {} 失败: {err}", file.display()))?
            .hash(&mut hasher);
    }
    Ok(hasher.finish())
}

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

async fn preview_homework(arguments: &serde_json::Value, course_id: &str, assignment_id: &str) -> Result<String, String> {
    let submission = submission_from_arguments(arguments, course_id, assignment_id)?
        .ok_or_else(|| "请提供要提交的内容：text（在线文本）或 files（本地文件路径）".to_string())?;
    let kind = submission.kind();

    let token = canvas_token()?;
    let client = reqwest::Client::new();
    let assignment = fetch_homework_assignment(&client, &token, course_id, assignment_id).await?;
    check_submittable(&assignment, kind, &submission.files)?;

    let notice = format!(
        "【作业提交预览，尚未提交】\n{}本次提交方式：{kind}\n{}\n请核对以上内容。确认无误后回复“提交”即可；不回复则不会提交。",
        assignment.summary(),
        submission.describe()
    );
    {
        let mut pending = pending_submissions().lock().unwrap();
        // 同一作业只保留最新一次预览
        pending.retain(|old| !old.expired() && !old.is_for(course_id, assignment_id));
        pending.push(submission);
    }

    Ok(format!(
        "提交预览已由系统直接展示给用户，作业尚未提交。\
         请简短提醒用户核对预览，然后结束本轮回答，等待用户回复。\
         用户明确表示可以提交后，用 action=submit、confirm=true 和相同的 course_id、assignment_id 提交，\
         不必再传内容，届时提交的就是预览过的这一份。用户没有明确同意前不要提交。\
         如需修改内容，请重新预览。{USER_NOTICE_MARK}{notice}"
    ))
}

async fn submit_homework(arguments: &serde_json::Value) -> Result<String, String> {
    let course_id = homework_arg(arguments, "course_id")
        .ok_or_else(|| "提交作业需要 course_id".to_string())?;
    let assignment_id = homework_arg(arguments, "assignment_id")
        .ok_or_else(|| "提交作业需要 assignment_id，可由 get_exam 获取".to_string())?;
    // confirm=true 表示用户已经明确同意提交；否则只生成预览，请用户过目。
    if arguments["confirm"].as_bool() != Some(true) {
        return preview_homework(arguments, course_id, assignment_id).await;
    }

    // 这次调用带了内容就直接交这份；没带内容就交之前预览过的那份。
    let fresh = submission_from_arguments(arguments, course_id, assignment_id)?;
    let submission = {
        let mut pending = pending_submissions().lock().unwrap();
        pending.retain(|submission| !submission.expired());
        let previewed = pending
            .iter()
            .position(|submission| submission.is_for(course_id, assignment_id))
            .map(|position| pending.remove(position));
        fresh.or(previewed).ok_or_else(|| {
            "没有可提交的内容：请传入 text 或 files；这份作业也没有仍然有效的预览".to_string()
        })?
    };
    if files_digest(&submission.files)? != submission.files_digest {
        return Err("提交文件在预览后被修改，作业未提交，请重新预览并让用户确认".to_string());
    }

    let kind = submission.kind();
    let token = canvas_token()?;
    let client = reqwest::Client::new();
    let assignment = fetch_homework_assignment(&client, &token, course_id, assignment_id).await?;
    check_submittable(&assignment, kind, &submission.files)?;

    let mut form: Vec<(&str, String)> = vec![("submission[submission_type]", kind.to_string())];
    if let Some(text) = &submission.text {
        form.push(("submission[body]", text_to_html(text)));
    }
    for file in &submission.files {
        let id = upload_submission_file(&client, &token, course_id, assignment_id, file).await?;
        form.push(("submission[file_ids][]", id.to_string()));
    }
    if let Some(comment) = &submission.comment {
        form.push(("comment[text_comment]", comment.clone()));
    }

    let response = client
        .post(format!(
            "https://oc.sjtu.edu.cn/api/v1/courses/{course_id}/assignments/{assignment_id}/submissions"
        ))
        .bearer_auth(&token)
        .form(&form)
        .send()
        .await
        .map_err(|err| format!("提交作业失败: {err}"))?;
    if !response.status().is_success() {
        return Err(canvas_failure(response, "提交作业").await);
    }
    let result: serde_json::Value = response
        .json()
        .await
        .map_err(|err| format!("解析提交结果失败: {err}"))?;

    let mut report = format!("已提交作业「{}」，提交方式：{kind}\n", assignment.name);
    for file in &submission.files {
        report.push_str(&format!("文件：{}\n", file.display()));
    }
    if let Some(text) = &submission.text {
        report.push_str(&format!("在线文本共 {} 字\n", text.chars().count()));
    }
    if let Some(time) = result["submitted_at"].as_str() {
        report.push_str(&format!("提交时间：{time}\n"));
    }
    if let Some(attempt) = result["attempt"].as_u64() {
        report.push_str(&format!("第 {attempt} 次提交\n"));
    }
    if result["late"].as_bool() == Some(true) {
        report.push_str("注意：本次提交已被标记为迟交\n");
    }
    if let Some(url) = result["preview_url"].as_str() {
        report.push_str(&format!("查看：{url}\n"));
    }
    Ok(report)
}

pub(super) async fn homework(arguments: serde_json::Value) -> Result<String, String> {
    match arguments["action"].as_str().unwrap_or("questions").trim() {
        "questions" => homework_questions(&arguments).await,
        "requirements" => requirements::read(&arguments).await,
        "submit" => submit_homework(&arguments).await,
        other => Err(format!("未知的 action「{other}」，只支持 questions / requirements / submit")),
    }
}

#[cfg(test)]
mod tests;
