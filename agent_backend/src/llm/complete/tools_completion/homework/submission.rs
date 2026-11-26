use std::collections::HashMap;
use std::hash::{BuildHasher, DefaultHasher, Hash, Hasher, RandomState};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::Value;

use super::super::{canvas::CANVAS_BASE, outbound::SESSION};
use super::{
    USER_NOTICE_MARK, artifact, canvas_failure, canvas_token, check_submittable, fetch_homework_assignment,
    homework_arg, text_to_html, upload_submission_file,
};

#[derive(PartialEq, Eq)]
struct Input {
    text: Option<String>,
    files: Vec<PathBuf>,
    comment: Option<String>,
}

impl Input {
    fn parse(args: &Value) -> Result<Option<Self>, String> {
        let text = homework_arg(args, "text").map(str::to_owned);
        let mut files = Vec::new();
        if let Some(values) = args.get("files").filter(|v| !v.is_null()) {
            for value in values.as_array().ok_or("files 应是文件路径列表")? {
                let path = value.as_str().map(str::trim).filter(|s| !s.is_empty()).ok_or("文件路径为空")?;
                files.push(PathBuf::from(path));
            }
        }
        if text.is_some() && !files.is_empty() {
            return Err("text 和 files 只能二选一：Canvas 一次接受一种提交方式".into());
        }
        if text.is_none() && files.is_empty() {
            return Ok(None);
        }
        artifact::check_required_files(args, &files)?;
        Ok(Some(Self { text, files, comment: homework_arg(args, "comment").map(str::to_owned) }))
    }

    fn kind(&self) -> &'static str {
        if self.text.is_some() { "online_text_entry" } else { "online_upload" }
    }
}

struct CheckedFile {
    snapshot: Option<std::fs::File>,
    digest: u64,
    summary: artifact::Inspection,
}

fn digest(bytes: &[u8]) -> u64 {
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

impl CheckedFile {
    fn open(path: &Path) -> Result<Self, String> {
        let bytes =
            std::fs::read(path).map_err(|err| format!("文件 {} 不存在或无法读取：{err}", path.display()))?;
        let suffix = path.extension().and_then(|s| s.to_str()).map(|s| format!(".{s}")).unwrap_or_default();
        let mut snapshot =
            tempfile::Builder::new().suffix(&suffix).tempfile().map_err(|err| err.to_string())?;
        snapshot.write_all(&bytes).map_err(|err| err.to_string())?;
        let digest = digest(&bytes);
        drop(bytes);
        let summary = artifact::inspect(snapshot.path())
            .map_err(|err| err.replace(snapshot.path().to_string_lossy().as_ref(), &path.to_string_lossy()))?;
        // Inspection needs an extension. Once inspected, keep only the open file handle.
        Ok(Self { snapshot: Some(snapshot.into_file()), digest, summary })
    }

    async fn bytes(&self) -> Result<Vec<u8>, String> {
        use tokio::io::{AsyncReadExt, AsyncSeekExt};
        let file =
            self.snapshot.as_ref().ok_or("提交文件快照已释放")?.try_clone().map_err(|e| e.to_string())?;
        let mut file = tokio::fs::File::from_std(file);
        file.rewind().await.map_err(|e| e.to_string())?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).await.map_err(|e| e.to_string())?;
        Ok(bytes)
    }
}

struct Attempt {
    previous: u64,
    uploaded: Vec<u64>,
    assignment: String,
    sent_at: Instant,
}

enum Delivery {
    Ready,
    AwaitingReceipt(Attempt),
    Done(String),
}

struct Draft {
    input: Input,
    files: Vec<CheckedFile>,
    preview_id: Option<String>,
    created: Instant,
    delivery: Delivery,
}

impl Draft {
    async fn new(input: Input, preview_id: Option<String>) -> Result<Self, String> {
        tokio::task::spawn_blocking(move || {
            let files = input.files.iter().map(|path| CheckedFile::open(path)).collect::<Result<_, _>>()?;
            Ok(Self { input, files, preview_id, created: Instant::now(), delivery: Delivery::Ready })
        })
        .await
        .map_err(|err| format!("检查提交文件失败：{err}"))?
    }

    async fn files_unchanged(&self) -> Result<bool, String> {
        for (path, checked) in self.input.files.iter().zip(&self.files) {
            let bytes = tokio::fs::read(path)
                .await
                .map_err(|err| format!("无法打开原文件 {}：{err}", path.display()))?;
            if digest(&bytes) != checked.digest {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn describe(&self, full_text: bool) -> String {
        let mut out = String::new();
        if let Some(text) = &self.input.text {
            out.push_str(&format!("在线文本共 {} 字\n", text.chars().count()));
            if full_text {
                out.push_str(text);
                out.push('\n');
            }
        }
        for (path, checked) in self.input.files.iter().zip(&self.files) {
            out.push_str(&format!("文件：{}（{}）\n", path.display(), checked.summary));
        }
        if let Some(comment) = &self.input.comment {
            out.push_str(&format!("附言：{comment}\n"));
        }
        out
    }

    fn signature(&self) -> u64 {
        let mut h = DefaultHasher::new();
        self.input.text.hash(&mut h);
        self.input.comment.hash(&mut h);
        for (path, checked) in self.input.files.iter().zip(&self.files) {
            path.file_name().hash(&mut h);
            checked.digest.hash(&mut h);
        }
        h.finish()
    }
}

struct Completed {
    signature: u64,
    attempt: u64,
    submitted_at: String,
    finished: Instant,
    report: String,
}

#[derive(Default)]
struct AssignmentDrafts {
    sessions: HashMap<String, Draft>,
    latest: Option<Completed>,
}

type Target = Arc<tokio::sync::Mutex<AssignmentDrafts>>;
const PREVIEW_LIFETIME: Duration = Duration::from_secs(30 * 60);

impl AssignmentDrafts {
    fn remove_expired(&mut self) {
        self.sessions.retain(|_, d| {
            d.created.elapsed() < PREVIEW_LIFETIME || matches!(d.delivery, Delivery::AwaitingReceipt(_))
        });
    }
}

fn assignment_drafts(course: &str, assignment: &str) -> Target {
    static TARGETS: OnceLock<Mutex<HashMap<(String, String), Target>>> = OnceLock::new();
    let mut targets = TARGETS.get_or_init(Default::default).lock().unwrap();
    targets.retain(|_, cell| {
        if Arc::strong_count(cell) > 1 {
            return true;
        }
        let Ok(mut drafts) = cell.try_lock() else {
            return true;
        };
        drafts.remove_expired();
        !drafts.sessions.is_empty()
    });
    targets.entry((course.to_owned(), assignment.to_owned())).or_default().clone()
}

#[derive(serde::Deserialize)]
struct Attachment {
    id: u64,
}

#[derive(serde::Deserialize)]
struct Record {
    assignment_id: u64,
    attempt: Option<u64>,
    submitted_at: Option<String>,
    workflow_state: String,
    submission_type: Option<String>,
    body: Option<String>,
    attachments: Option<Vec<Attachment>>,
    late: Option<bool>,
    preview_url: Option<String>,
}

impl Record {
    fn matches(&self, draft: &Draft, attempt: &Attempt, assignment_id: u64) -> bool {
        if self.assignment_id != assignment_id
            || self.attempt.unwrap_or(0) <= attempt.previous
            || self.submission_type.as_deref() != Some(draft.input.kind())
            || !matches!(self.workflow_state.as_str(), "submitted" | "graded" | "pending_review")
            || !self.submitted_at.as_deref().is_some_and(|s| chrono::DateTime::parse_from_rfc3339(s).is_ok())
        {
            return false;
        }
        if let Some(text) = &draft.input.text {
            // Canvas may serialize a line break as XHTML or normalize its following newline.
            let normalize = |body: &str| {
                body.trim()
                    .replace("\r\n", "\n")
                    .replace("<br />", "<br>")
                    .replace("<br/>", "<br>")
                    .replace("<br>\n", "<br>")
                    .replace("&nbsp;", "\u{a0}")
            };
            return self.body.as_deref().is_some_and(|body| normalize(body) == normalize(&text_to_html(text)));
        }
        let mut received: Vec<_> = self.attachments.iter().flatten().map(|f| f.id).collect();
        let mut expected = attempt.uploaded.clone();
        received.sort_unstable();
        expected.sort_unstable();
        received == expected
    }

    fn finish(&self, draft: &mut Draft, name: &str) -> Completed {
        let number = self.attempt.unwrap_or_default();
        let time = self.submitted_at.as_deref().unwrap_or_default();
        let mut report =
            format!("已提交作业「{name}」，第 {number} 次提交\n提交时间：{time}\n{}", draft.describe(false));
        if self.late == Some(true) {
            report.push_str("Canvas 已标记为迟交\n");
        }
        if let Some(url) = &self.preview_url {
            report.push_str(&format!("查看：{url}\n"));
        }
        draft.delivery = Delivery::Done(report.clone());
        for file in &mut draft.files {
            file.snapshot = None;
        }
        draft.created = Instant::now();
        Completed {
            signature: draft.signature(),
            attempt: number,
            submitted_at: time.into(),
            finished: draft.created,
            report,
        }
    }
}

struct CanvasTarget<'a> {
    client: reqwest::Client,
    course: &'a str,
    assignment: &'a str,
    assignment_id: u64,
    url: String,
}

impl CanvasTarget<'_> {
    async fn own_record(&self, token: &str) -> Result<Record, String> {
        let assignment_id = self.assignment_id;
        let response = self
            .client
            .get(format!("{}/self", self.url))
            .bearer_auth(token)
            .send()
            .await
            .map_err(|err| format!("查询本人 Canvas 提交记录失败：{err}"))?;
        // Canvas normally returns an unsubmitted record on a first attempt. A 404 alone is ambiguous.
        let record: Record = if response.status() == reqwest::StatusCode::NOT_FOUND {
            let assignment = self
                .client
                .get(self.url.trim_end_matches("/submissions"))
                .query(&[("include[]", "submission")])
                .bearer_auth(token)
                .send()
                .await
                .map_err(|err| err.to_string())?;
            if !assignment.status().is_success() {
                return Err(canvas_failure(assignment, "核对作业中的本人提交记录").await);
            }
            let data: Value = assignment.json().await.map_err(|err| err.to_string())?;
            if data["id"].as_u64() != Some(assignment_id) {
                return Err("Canvas 作业编号不匹配".into());
            }
            match data.get("submission") {
                Some(Value::Null) => Record {
                    assignment_id,
                    attempt: None,
                    submitted_at: None,
                    workflow_state: "unsubmitted".into(),
                    submission_type: None,
                    body: None,
                    attachments: None,
                    late: None,
                    preview_url: None,
                },
                Some(record) => serde_json::from_value(record.clone())
                    .map_err(|err| format!("无法核对作业中的本人提交记录：{err}"))?,
                None => return Err("Canvas 没有提供本人提交记录，暂时无法核对提交次数".into()),
            }
        } else {
            if !response.status().is_success() {
                return Err(canvas_failure(response, "查询本人提交记录").await);
            }
            response.json().await.map_err(|err| format!("无法解析本人提交记录：{err}"))?
        };
        if record.assignment_id != assignment_id {
            return Err("Canvas 返回的本人提交记录对应另一份作业".into());
        }
        if record.submitted_at.is_some() && record.attempt.unwrap_or(0) == 0 {
            return Err("Canvas 已有提交时间，却没有返回对应的提交次数，请核对本人记录".into());
        }
        Ok(record)
    }

    async fn deliver(&self, draft: &mut Draft, latest: &mut Option<Completed>) -> Result<String, String> {
        let token = canvas_token()?;
        let metadata = fetch_homework_assignment(&self.client, &token, self.course, self.assignment).await?;
        check_submittable(&metadata, draft.input.kind(), &draft.input.files)?;
        let previous = self.own_record(&token).await?;
        if latest.as_ref().is_some_and(|done| {
            done.signature == draft.signature()
                && previous.attempt == Some(done.attempt)
                && previous.submitted_at.as_deref() == Some(&done.submitted_at)
                && (draft.preview_id.is_none() || done.finished >= draft.created)
        }) {
            return Ok(previous.finish(draft, &metadata.name).report);
        }
        let mut form = vec![("submission[submission_type]", draft.input.kind().to_owned())];
        if let Some(text) = &draft.input.text {
            form.push(("submission[body]", text_to_html(text)));
        }
        let mut uploaded = Vec::new();
        for (path, checked) in draft.input.files.iter().zip(&draft.files) {
            let id = upload_submission_file(
                &self.client,
                &token,
                self.course,
                self.assignment,
                path,
                checked.bytes().await?,
            )
            .await?;
            form.push(("submission[file_ids][]", id.to_string()));
            uploaded.push(id);
        }
        if let Some(comment) = &draft.input.comment {
            form.push(("comment[text_comment]", comment.clone()));
        }
        // Record the possibly delivered POST before awaiting it, including if the chat is cancelled.
        draft.delivery = Delivery::AwaitingReceipt(Attempt {
            previous: previous.attempt.unwrap_or(0),
            uploaded,
            assignment: metadata.name.clone(),
            sent_at: Instant::now(),
        });
        let Delivery::AwaitingReceipt(sent) = &draft.delivery else { unreachable!() };
        let response = self.client.post(&self.url).bearer_auth(&token).form(&form).send().await;
        let post_receipt = match response {
            Ok(r) if r.status().is_success() => r.json::<Record>().await.ok(),
            Ok(r) if r.status().is_client_error() && r.status() != reqwest::StatusCode::REQUEST_TIMEOUT => {
                draft.delivery = Delivery::Ready;
                return Err(canvas_failure(r, "提交作业").await);
            }
            _ => None,
        };
        let receipt = match post_receipt.filter(|r| r.matches(draft, sent, self.assignment_id)) {
            Some(r) => r,
            None => {
                let r = self
                    .own_record(&token)
                    .await
                    .map_err(|err| format!("提交已发出，结果待核实。再次确认将先查询回执。{err}"))?;
                if !r.matches(draft, sent, self.assignment_id) {
                    return Err("提交已发出，Canvas 尚无对应本次内容的新回执。再次确认只查询状态。".into());
                }
                r
            }
        };
        let completed = receipt.finish(draft, &metadata.name);
        let report = completed.report.clone();
        *latest = Some(completed);
        Ok(report)
    }
}

fn preview_message(draft: &Draft, assignment: &super::HomeworkAssignment, id: &str, session: &str) -> String {
    let details = format!(
        "【作业提交预览】\n{}本次提交方式：{}\n{}",
        assignment.summary(),
        draft.input.kind(),
        draft.describe(true)
    );
    if session == crate::HOMEWORK_CLI_SESSION {
        return format!(
            "{details}\n单次命令退出后预览即结束。要提交这份内容，请再次调用 --tool homework，提供原正文或文件并设置 confirm=true。"
        );
    }
    format!(
        "预览已直接展示给用户。用户同意后，以 confirm=true 和 preview_id={id} 确认此版本，沿用相同的 course_id、assignment_id，无需传正文或文件，也无需用户输入 ID。当前先等待用户回复。{USER_NOTICE_MARK}{details}\n核对后回复“提交”即可。预览保留 30 分钟。"
    )
}

enum Selection {
    New,
    Current,
    Completed(String),
}

async fn select_draft(
    current: Option<&Draft>,
    incoming: Option<&Input>,
    preview_id: Option<&str>,
    comment: Option<&str>,
) -> Result<Selection, String> {
    if preview_id.is_some() && current.and_then(|d| d.preview_id.as_deref()) != preview_id {
        return Err("此预览已过期、被替换或属于其他会话。请核对本会话中用户确认的那份预览。".into());
    }
    let Some(draft) = current else {
        return Ok(Selection::New);
    };
    let same = incoming.is_none_or(|other| other == &draft.input)
        && comment.is_none_or(|c| Some(c) == draft.input.comment.as_deref());
    if let Delivery::Done(report) = &draft.delivery {
        if same && (incoming.is_none() || preview_id.is_some() || draft.files_unchanged().await?) {
            return Ok(Selection::Completed(report.clone()));
        }
        if preview_id.is_some() {
            return Err("此次内容与已经提交的预览不同，请核对要提交的新版本".into());
        }
        return Ok(Selection::New);
    }
    if draft.preview_id.is_some() && preview_id.is_none() {
        return Err("当前会话有待确认的预览。请从本会话相应的 homework 预览结果取回 preview_id，按用户已给出的确认重新调用。用户无需抄写 ID。".into());
    }
    if !same {
        return Err("本次内容与当前草稿不同。要交原稿，请沿用原确认。要交修改稿，请重新预览。".into());
    }
    if matches!(draft.delivery, Delivery::Ready) && !draft.files_unchanged().await? {
        return Err("提交文件在预览后被修改，请重新预览修改后的文件".into());
    }
    Ok(Selection::Current)
}

pub(super) async fn submit_homework(args: &Value) -> Result<String, String> {
    let course = homework_arg(args, "course_id").ok_or("提交作业需要 course_id")?;
    let assignment =
        homework_arg(args, "assignment_id").ok_or("提交作业需要 assignment_id，可由 get_exam 获取")?;
    let assignment_id = assignment.parse::<u64>().map_err(|_| "assignment_id 应为数字")?;
    if course.parse::<u64>().is_err() {
        return Err("course_id 应为数字".into());
    }
    let confirm = args["confirm"].as_bool() == Some(true);
    let preview_id = homework_arg(args, "preview_id");
    let incoming = Input::parse(args)?;
    if !confirm && incoming.is_none() {
        return Err("请提供 text（在线文本）或 files（本地文件路径）".into());
    }
    let session = SESSION.try_with(Clone::clone).map_err(|_| "请在聊天会话内提交作业")?;
    let target = assignment_drafts(course, assignment);
    let mut guard = target.lock().await;
    guard.remove_expired();
    let AssignmentDrafts { sessions, latest } = &mut *guard;
    let canvas = CanvasTarget {
        client: reqwest::Client::new(),
        course,
        assignment,
        assignment_id,
        url: format!("{CANVAS_BASE}/api/v1/courses/{course}/assignments/{assignment}/submissions"),
    };
    let selected = if confirm {
        Some(
            select_draft(sessions.get(&session), incoming.as_ref(), preview_id, homework_arg(args, "comment"))
                .await?,
        )
    } else {
        None
    };
    if let Some(draft) = sessions.get_mut(&session) {
        if let Delivery::AwaitingReceipt(sent) = &draft.delivery {
            let record = canvas
                .own_record(&canvas_token()?)
                .await
                .map_err(|err| format!("上次提交结果仍待核实，本次仅查询了回执。{err}"))?;
            if record.matches(draft, sent, assignment_id) {
                let name = sent.assignment.clone();
                let completed = record.finish(draft, &name);
                let report = completed.report.clone();
                *latest = Some(completed);
                if confirm {
                    return Ok(report);
                }
            } else if confirm {
                return Err(format!(
                    "上次提交结果仍待核实。Canvas 当前记录为第 {} 次，本次仅查询了回执。用户明确要重新提交时，先生成一份新预览。",
                    record.attempt.unwrap_or(0)
                ));
            }
        }
    }
    if !confirm {
        let id = format!("{:016x}", RandomState::new().hash_one(&session));
        let draft = Draft::new(incoming.ok_or("缺少待预览内容")?, Some(id.clone())).await?;
        artifact::check_required_questions(
            args,
            draft.input.text.as_deref(),
            draft.files.iter().map(|f| &f.summary),
        )?;
        let metadata = fetch_homework_assignment(&canvas.client, &canvas_token()?, course, assignment).await?;
        check_submittable(&metadata, draft.input.kind(), &draft.input.files)?;
        let mut notice = preview_message(&draft, &metadata, &id, &session);
        if sessions
            .values()
            .any(|d| matches!(d.delivery, Delivery::AwaitingReceipt(_)) && d.signature() == draft.signature())
        {
            notice.push_str("\n相同内容上次提交的回执尚待核实。若明确需要重交，再确认本次预览。");
        }
        if session == crate::HOMEWORK_CLI_SESSION {
            return Ok(notice);
        }
        sessions.insert(session, draft);
        return Ok(notice);
    }
    if let Some(Selection::Completed(report)) = selected {
        return Ok(report);
    }
    if matches!(selected, Some(Selection::New)) {
        let input = incoming.ok_or("没有可提交的内容：请传入 text 或 files，或确认本会话中有效的预览")?;
        sessions.insert(session.clone(), Draft::new(input, None).await?);
    }
    let current = &sessions[&session];
    artifact::check_required_files(args, &current.input.files)?;
    artifact::check_required_questions(
        args,
        current.input.text.as_deref(),
        current.files.iter().map(|f| &f.summary),
    )?;
    let signature = current.signature();
    let previewed_at = current.preview_id.as_ref().map(|_| current.created);
    if let Some((_, other)) = sessions.iter_mut().find(|(key, d)| {
        *key != &session && matches!(d.delivery, Delivery::AwaitingReceipt(_)) && d.signature() == signature
    }) {
        let Delivery::AwaitingReceipt(sent) = &other.delivery else { unreachable!() };
        let record = canvas.own_record(&canvas_token()?).await?;
        if record.matches(other, sent, assignment_id) {
            let name = sent.assignment.clone();
            *latest = Some(record.finish(other, &name));
            return Ok(record.finish(sessions.get_mut(&session).expect("selected draft"), &name).report);
        }
        if !previewed_at.is_some_and(|at| at > sent.sent_at) {
            return Err("同一作业已有相同内容等待 Canvas 回执。本次仅查询了状态。".into());
        }
    }
    let draft = sessions.get_mut(&session).expect("selected or inserted draft");
    canvas.deliver(draft, latest).await
}

#[tokio::test]
async fn homework_submits_directly_or_from_the_latest_preview() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("answer.txt");
    std::fs::write(&path, "version one").unwrap();
    let args = serde_json::json!({"files": [path]});
    let input = Input::parse(&args).unwrap().unwrap();
    assert!(matches!(select_draft(None, Some(&input), None, None).await, Ok(Selection::New)));
    let draft = Draft::new(input, Some("v2".into())).await.unwrap();
    assert!(select_draft(None, None, Some("v2"), None).await.is_err());
    assert!(select_draft(Some(&draft), None, Some("v1"), None).await.is_err());
    assert!(select_draft(Some(&draft), None, None, None).await.is_err());
    assert!(matches!(select_draft(Some(&draft), None, Some("v2"), None).await, Ok(Selection::Current)));
    let changed = Input::parse(&serde_json::json!({"text":"different"})).unwrap().unwrap();
    assert!(select_draft(Some(&draft), Some(&changed), Some("v2"), None).await.is_err());
    std::fs::write(&path, "version two").unwrap();
    assert!(select_draft(Some(&draft), None, Some("v2"), None).await.is_err());
    assert_eq!(draft.files[0].bytes().await.unwrap(), b"version one");
}
