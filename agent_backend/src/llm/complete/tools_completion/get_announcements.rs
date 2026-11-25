//! get_announcements：各门课最近的 Canvas 公告，按发布时间倒序。

use std::collections::HashMap;

use chrono::{DateTime, Duration, FixedOffset};
use serde_json::Value;

use super::campus_time::{self, fmt_day_time, parse_time};
use super::canvas::{Canvas, null_default};
use super::clip;
use super::text::{html_to_text, shorten};

const DEFAULT_DAYS: i64 = 14;
const MAX_DAYS: i64 = 180;
const MAX_ITEMS: usize = 20;

#[derive(serde::Deserialize)]
pub(super) struct Course {
    id: u64,
    #[serde(default)]
    name: String,
}

#[derive(serde::Deserialize)]
pub(super) struct Announcement {
    #[serde(default)]
    title: String,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    posted_at: Option<String>,
    #[serde(default)]
    context_code: String,
    #[serde(default)]
    user_name: Option<String>,
    #[serde(default)]
    author: Option<Author>,
    #[serde(default)]
    read_state: Option<String>,
    #[serde(default)]
    html_url: Option<String>,
    #[serde(default, deserialize_with = "null_default")]
    attachments: Vec<Attachment>,
}

#[derive(serde::Deserialize)]
struct Author {
    #[serde(default)]
    display_name: Option<String>,
}

#[derive(serde::Deserialize)]
struct Attachment {
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    filename: Option<String>,
}

impl Announcement {
    fn posted(&self) -> Option<DateTime<FixedOffset>> {
        self.posted_at.as_deref().and_then(parse_time)
    }

    fn author(&self) -> Option<&str> {
        self.author
            .as_ref()
            .and_then(|author| author.display_name.as_deref())
            .or(self.user_name.as_deref())
            .filter(|name| !name.trim().is_empty())
    }
}

pub(super) fn render_announcements(
    announcements: &mut [Announcement],
    courses: &HashMap<String, String>,
    canvas: &Canvas,
    now: DateTime<FixedOffset>,
    days: i64,
) -> String {
    if announcements.is_empty() {
        return format!("最近 {days} 天没有新公告。");
    }
    announcements.sort_by_key(|announcement| std::cmp::Reverse(announcement.posted()));
    let unread = announcements
        .iter()
        .filter(|announcement| announcement.read_state.as_deref() == Some("unread"))
        .count();
    let mut text = format!(
        "最近 {days} 天共 {} 条公告（其中 Canvas 标记未读 {unread} 条），按发布时间从新到旧：\n",
        announcements.len()
    );
    // 条数少时给全文，条数多时给摘要，避免一次塞满上下文
    let body_chars = if announcements.len() <= 3 { 2000 } else if announcements.len() <= 8 { 500 } else { 200 };
    for announcement in announcements.iter().take(MAX_ITEMS) {
        let course = courses
            .get(&announcement.context_code)
            .map(String::as_str)
            .unwrap_or(announcement.context_code.as_str());
        let mut meta = vec![announcement
            .posted()
            .map_or("时间未知".to_string(), |time| fmt_day_time(time, now))];
        if let Some(author) = announcement.author() {
            meta.push(author.to_string());
        }
        if announcement.read_state.as_deref() == Some("unread") {
            meta.push("未读".into());
        }
        text.push_str(&format!(
            "\n【{course}】{}（{}）\n",
            announcement.title.trim(),
            meta.join("，")
        ));
        let body = html_to_text(announcement.message.as_deref().unwrap_or_default());
        let body = body.lines().map(str::trim).filter(|line| !line.is_empty()).collect::<Vec<_>>().join("\n");
        if !body.is_empty() {
            text.push_str(&shorten(&body, body_chars));
            text.push('\n');
        }
        let files: Vec<&str> = announcement
            .attachments
            .iter()
            .filter_map(|file| file.display_name.as_deref().or(file.filename.as_deref()))
            .collect();
        if !files.is_empty() {
            text.push_str(&format!("附件：{}（可用 course_files 下载）\n", files.join("、")));
        }
        if let Some(url) = &announcement.html_url {
            text.push_str(&format!("链接：{}\n", canvas.url(url)));
        }
    }
    if announcements.len() > MAX_ITEMS {
        text.push_str(&format!(
            "\n另有 {} 条更早的公告未列出，可缩小 days 或指定 course_id 查看。\n",
            announcements.len() - MAX_ITEMS
        ));
    }
    text
}

fn window_days(arguments: &Value) -> Result<i64, String> {
    match arguments.get("days") {
        None | Some(Value::Null) => Ok(DEFAULT_DAYS),
        Some(value) => value
            .as_i64()
            .filter(|days| (1..=MAX_DAYS).contains(days))
            .ok_or_else(|| format!("days 必须是 1 到 {MAX_DAYS} 之间的整数")),
    }
}

pub(super) async fn get_announcements(arguments: Value) -> Result<String, String> {
    let days = window_days(&arguments)?;
    let course_id = match &arguments["course_id"] {
        Value::String(id) if !id.trim().is_empty() => Some(id.trim().to_string()),
        Value::Number(id) => Some(id.to_string()),
        _ => None,
    };
    if course_id.as_ref().is_some_and(|id| !id.chars().all(|c| c.is_ascii_digit())) {
        return Err("course_id 应为数字，可先用 get_courses 查询".into());
    }
    let canvas = Canvas::from_env()?;
    let courses: Vec<Course> = canvas
        .get_all(
            "/api/v1/courses",
            &[("enrollment_state", "active".into())],
            "课程列表",
        )
        .await?;
    let names: HashMap<String, String> = courses
        .iter()
        .map(|course| (format!("course_{}", course.id), course.name.clone()))
        .collect();
    let codes: Vec<String> = match &course_id {
        Some(id) => vec![format!("course_{id}")],
        None => courses.iter().map(|course| format!("course_{}", course.id)).collect(),
    };
    if codes.is_empty() {
        return Ok("没有进行中的课程。".into());
    }
    let now = campus_time::now();
    let mut query: Vec<(&str, String)> = codes.into_iter().map(|code| ("context_codes[]", code)).collect();
    query.push(("start_date", (now - Duration::days(days)).to_rfc3339()));
    query.push(("end_date", (now + Duration::days(1)).to_rfc3339()));
    let mut announcements: Vec<Announcement> = canvas.get_all("/api/v1/announcements", &query, "课程公告").await?;
    Ok(clip(&render_announcements(&mut announcements, &names, &canvas, now, days)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use serde_json::json;

    #[test]
    fn announcements_are_newest_first_with_course_names_and_attachments() {
        let now = campus_time::beijing().with_ymd_and_hms(2026, 11, 25, 12, 0, 0).unwrap();
        let mut announcements: Vec<Announcement> = serde_json::from_value(json!([
            {"title": "期中成绩已公布", "message": "<p>请登录查看。</p><p>有疑问联系助教。</p>",
             "posted_at": "2026-11-20T02:00:00Z", "context_code": "course_1", "user_name": "王老师",
             "read_state": "read", "html_url": "/courses/1/discussion_topics/5", "attachments": null},
            {"title": "调课通知", "message": "<p>下周四的课改到周五 3-4 节，地点东上院 102。</p>",
             "posted_at": "2026-11-25T01:30:00Z", "context_code": "course_2",
             "author": {"display_name": "李老师"}, "read_state": "unread",
             "attachments": [{"display_name": "调课安排.pdf"}]}
        ]))
        .unwrap();
        let names = HashMap::from([
            ("course_1".to_string(), "线性代数".to_string()),
            ("course_2".to_string(), "大学物理".to_string()),
        ]);
        let canvas = Canvas::new(super::super::canvas::CANVAS_BASE, "token".into());
        let text = render_announcements(&mut announcements, &names, &canvas, now, 14);
        assert!(text.contains("共 2 条公告（其中 Canvas 标记未读 1 条）"), "{text}");
        let newest = text.find("【大学物理】调课通知（今天 周三 09:30，李老师，未读）").unwrap();
        let older = text.find("【线性代数】期中成绩已公布（11月20日 周五 10:00，王老师）").unwrap();
        assert!(newest < older, "{text}");
        assert!(text.contains("请登录查看。\n有疑问联系助教。"), "{text}");
        assert!(text.contains("附件：调课安排.pdf"));
        assert!(text.contains("链接：https://oc.sjtu.edu.cn/courses/1/discussion_topics/5"));
        assert_eq!(render_announcements(&mut [], &names, &canvas, now, 7), "最近 7 天没有新公告。");
    }

    #[tokio::test]
    async fn invalid_arguments_fail_before_any_request() {
        assert!(get_announcements(json!({"days": 0})).await.unwrap_err().contains("days"));
        assert!(get_announcements(json!({"course_id": "物理"})).await.unwrap_err().contains("course_id"));
    }
}
