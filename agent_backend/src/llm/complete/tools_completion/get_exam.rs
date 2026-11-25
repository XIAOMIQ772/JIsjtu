//! get_exam：一门课的全部作业，按截止时间整理，附带自己的提交状态。
//!
//! 作业列表接口本身就带说明、提交方式和开放时间，一次分页请求即可，
//! 不再逐个作业请求详情。

use chrono::{DateTime, FixedOffset};

use super::campus_time::{self, describe_deadline, fmt_time, parse_time};
use super::canvas::{Canvas, Submission, null_default, submission_type_cn, trim_number};
use super::clip;
use super::text::{html_to_text, one_line, shorten};

const DESCRIPTION_CHARS: usize = 300;

#[derive(serde::Deserialize)]
pub(super) struct Assignment {
    id: u64,
    #[serde(default)]
    name: String,
    #[serde(default)]
    due_at: Option<String>,
    #[serde(default)]
    lock_at: Option<String>,
    #[serde(default)]
    unlock_at: Option<String>,
    #[serde(default)]
    points_possible: Option<f64>,
    #[serde(default)]
    html_url: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default, deserialize_with = "null_default")]
    submission_types: Vec<String>,
    #[serde(default, deserialize_with = "null_default")]
    allowed_extensions: Vec<String>,
    #[serde(default)]
    allowed_attempts: Option<i64>,
    #[serde(default, deserialize_with = "null_default")]
    locked_for_user: bool,
    #[serde(default)]
    submission: Option<Submission>,
}

impl Assignment {
    fn due(&self) -> Option<DateTime<FixedOffset>> {
        self.due_at.as_deref().and_then(parse_time)
    }

    fn needs_submission(&self) -> bool {
        !self.submission_types.is_empty()
            && !self
                .submission_types
                .iter()
                .all(|kind| matches!(kind.as_str(), "none" | "on_paper" | "not_graded"))
    }

    fn is_done(&self) -> bool {
        self.submission.as_ref().is_some_and(Submission::is_done)
    }

    /// 还需要自己动手交的作业：需要在线提交、还没交、也没被锁定。
    fn is_pending(&self, now: DateTime<FixedOffset>) -> bool {
        let locked = self.locked_for_user
            || self
                .lock_at
                .as_deref()
                .and_then(parse_time)
                .is_some_and(|lock| lock <= now);
        self.needs_submission() && !self.is_done() && !locked
    }

    fn render(&self, canvas: &Canvas, now: DateTime<FixedOffset>) -> String {
        let mut text = format!("\n[{}] {}\n", self.id, self.name.trim());
        let due = self.due();
        match &self.due_at {
            Some(raw) => text.push_str(&format!("  截止：{}\n", describe_deadline(raw, now))),
            None => text.push_str("  截止：未设置\n"),
        }
        let status = if self.needs_submission() {
            self.submission
                .clone()
                .unwrap_or_default()
                .status(self.points_possible, due.is_some_and(|due| due < now), now)
        } else {
            "无需在线提交".to_string()
        };
        text.push_str(&format!("  状态：{status}\n"));

        let mut facts = Vec::new();
        if let Some(points) = self.points_possible.filter(|points| *points > 0.0) {
            facts.push(format!("分值 {}", trim_number(points)));
        }
        if self.needs_submission() {
            let kinds: Vec<&str> = self.submission_types.iter().map(|kind| submission_type_cn(kind)).collect();
            let mut kinds = kinds.join("/");
            if !self.allowed_extensions.is_empty() {
                kinds.push_str(&format!("（{}）", self.allowed_extensions.join("、")));
            }
            facts.push(format!("提交方式 {kinds}"));
        }
        match self.allowed_attempts {
            Some(attempts) if attempts > 0 => facts.push(format!("可提交 {attempts} 次")),
            _ => {}
        }
        if !facts.is_empty() {
            text.push_str(&format!("  {}\n", facts.join(" · ")));
        }
        if let Some(unlock) = self.unlock_at.as_deref().and_then(parse_time).filter(|unlock| *unlock > now) {
            text.push_str(&format!("  开放时间：{}\n", fmt_time(unlock, now)));
        }
        if let Some(lock) = self.lock_at.as_deref().and_then(parse_time) {
            if due.is_none_or(|due| lock > due) {
                text.push_str(&format!("  最晚可交：{}\n", fmt_time(lock, now)));
            }
        }
        if let Some(url) = &self.html_url {
            text.push_str(&format!("  链接：{}\n", canvas.url(url)));
        }
        let description = one_line(&html_to_text(self.description.as_deref().unwrap_or_default()));
        if !description.is_empty() {
            text.push_str(&format!("  说明：{}\n", shorten(&description, DESCRIPTION_CHARS)));
        }
        text
    }
}

pub(super) fn render_assignments(
    course_id: &str,
    assignments: &[Assignment],
    canvas: &Canvas,
    now: DateTime<FixedOffset>,
    pending_only: bool,
) -> String {
    if assignments.is_empty() {
        return format!("课程 {course_id} 还没有公布作业。");
    }
    let mut upcoming: Vec<&Assignment> = Vec::new();
    let mut undated: Vec<&Assignment> = Vec::new();
    let mut past: Vec<&Assignment> = Vec::new();
    for assignment in assignments {
        if pending_only && !assignment.is_pending(now) {
            continue;
        }
        match assignment.due() {
            Some(due) if due >= now => upcoming.push(assignment),
            Some(_) => past.push(assignment),
            None => undated.push(assignment),
        }
    }
    upcoming.sort_by_key(|assignment| assignment.due());
    past.sort_by_key(|assignment| std::cmp::Reverse(assignment.due()));

    let todo_upcoming = upcoming.iter().filter(|a| a.is_pending(now)).count();
    let todo_past = past.iter().filter(|a| a.is_pending(now)).count();
    let mut text = if pending_only {
        format!(
            "课程 {course_id} 还需要提交的作业共 {} 项（未截止 {}，无截止时间 {}，已截止但仍可补交 {}）\n",
            upcoming.len() + undated.len() + past.len(),
            upcoming.len(),
            undated.len(),
            past.len()
        )
    } else {
        format!(
            "课程 {course_id} 共 {} 项作业：未截止 {} 项（其中待提交 {}），无截止时间 {} 项，已截止 {} 项（其中仍可补交 {}）\n",
            assignments.len(),
            upcoming.len(),
            todo_upcoming,
            undated.len(),
            past.len(),
            todo_past
        )
    };
    text.push_str(&format!(
        "（时间均为北京时间，当前 {}）\n",
        campus_time::fmt_time(now, now)
    ));
    for (title, group) in [("未截止", &upcoming), ("无截止时间", &undated), ("已截止", &past)] {
        if group.is_empty() {
            continue;
        }
        text.push_str(&format!("\n—— {title} ——"));
        for assignment in group.iter() {
            text.push_str(&assignment.render(canvas, now));
        }
    }
    if pending_only && upcoming.is_empty() && undated.is_empty() && past.is_empty() {
        text.push_str("\n没有需要提交的作业。");
    }
    text.push_str("\n读取某项作业的完整题目可用 homework 工具（action=questions，传 course_id 和 assignment_id）。");
    text
}

fn course_id(arguments: &serde_json::Value) -> Result<String, String> {
    match &arguments["course_id"] {
        serde_json::Value::String(id) if !id.trim().is_empty() => Ok(id.trim().to_string()),
        serde_json::Value::Number(id) => Ok(id.to_string()),
        _ => Err("get_exam 需要 course_id，可先用 get_courses 查询".to_string()),
    }
}

pub(super) async fn get_exam(arguments: serde_json::Value) -> Result<String, String> {
    let course_id = course_id(&arguments)?;
    if !course_id.chars().all(|c| c.is_ascii_digit()) {
        return Err(format!("course_id 应为数字，收到「{course_id}」；可先用 get_courses 查询"));
    }
    let pending_only = arguments["pending_only"].as_bool().unwrap_or(false);
    let canvas = Canvas::from_env()?;
    let assignments: Vec<Assignment> = canvas
        .get_all(
            &format!("/api/v1/courses/{course_id}/assignments"),
            &[("include[]", "submission".into()), ("order_by", "due_at".into())],
            &format!("课程 {course_id} 的作业"),
        )
        .await?;
    Ok(clip(&render_assignments(
        &course_id,
        &assignments,
        &canvas,
        campus_time::now(),
        pending_only,
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use serde_json::json;

    #[test]
    fn assignments_are_grouped_by_deadline_with_status_in_beijing_time() {
        let now = campus_time::beijing().with_ymd_and_hms(2026, 11, 25, 12, 0, 0).unwrap();
        let assignments: Vec<Assignment> = serde_json::from_value(json!([
            {"id": 1, "name": "HW1", "due_at": "2026-11-10T15:59:00Z", "points_possible": 10.0,
             "submission_types": ["online_upload"], "allowed_extensions": ["pdf"], "html_url": "/courses/9/assignments/1",
             "description": "<p>Solve <b>all</b> problems</p>",
             "submission": {"workflow_state": "graded", "submitted_at": "2026-11-09T12:00:00Z", "score": 9.0, "grade": "9", "late": false, "missing": false}},
            {"id": 2, "name": "HW2", "due_at": "2026-11-28T15:59:00Z", "points_possible": 10.0,
             "submission_types": ["online_text_entry", "online_upload"], "allowed_extensions": null,
             "submission": {"workflow_state": "unsubmitted", "late": false, "missing": false}},
            {"id": 3, "name": "Reading", "due_at": null, "submission_types": ["none"], "locked_for_user": null},
            {"id": 4, "name": "HW0", "due_at": "2026-11-01T15:59:00Z", "lock_at": "2026-12-01T15:59:00Z",
             "submission_types": ["online_upload"], "submission": {"workflow_state": "unsubmitted", "missing": true, "late": false}},
        ]))
        .unwrap();
        let canvas = Canvas::new(super::super::canvas::CANVAS_BASE, "token".into());
        let text = render_assignments("9", &assignments, &canvas, now, false);
        assert!(text.contains("共 4 项作业：未截止 1 项（其中待提交 1），无截止时间 1 项，已截止 2 项（其中仍可补交 1）"), "{text}");
        let upcoming = text.find("[2] HW2").unwrap();
        let undated = text.find("[3] Reading").unwrap();
        let recent_past = text.find("[1] HW1").unwrap();
        let old_past = text.find("[4] HW0").unwrap();
        assert!(upcoming < undated && undated < recent_past && recent_past < old_past, "{text}");
        assert!(text.contains("截止：11月28日 周六 23:59（还剩 3 天 11 小时）"), "{text}");
        assert!(text.contains("状态：已批改 9/10"));
        assert!(text.contains("提交方式 上传文件（pdf）"));
        assert!(text.contains("提交方式 在线文本/上传文件"));
        assert!(text.contains("状态：无需在线提交"));
        assert!(text.contains("状态：缺交"));
        assert!(text.contains("最晚可交：12月1日 周二 23:59"));
        assert!(text.contains("链接：https://oc.sjtu.edu.cn/courses/9/assignments/1"));
        assert!(text.contains("说明：Solve all problems"));

        let pending = render_assignments("9", &assignments, &canvas, now, true);
        assert!(pending.contains("[2] HW2") && pending.contains("[4] HW0"), "{pending}");
        assert!(!pending.contains("[1] HW1") && !pending.contains("[3] Reading"), "{pending}");
    }

    #[tokio::test]
    async fn bad_course_ids_are_rejected_without_a_request() {
        assert!(get_exam(json!({})).await.unwrap_err().contains("course_id"));
        assert!(get_exam(json!({"course_id": "高数"})).await.unwrap_err().contains("应为数字"));
    }
}
