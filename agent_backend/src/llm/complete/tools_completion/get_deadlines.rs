//! get_deadlines：跨所有课程的近期截止事项（Canvas 计划器），一次请求汇总。

use chrono::{DateTime, Duration, FixedOffset};
use serde_json::Value;

use super::campus_time::{self, fmt_day_time, fmt_remaining, parse_time};
use super::canvas::{Canvas, null_default, trim_number};
use super::clip;

const DEFAULT_DAYS: i64 = 14;
const MAX_DAYS: i64 = 60;
/// 回看多少天内逾期未交的作业
const OVERDUE_DAYS: i64 = 14;

#[derive(serde::Deserialize)]
pub(super) struct PlannerItem {
    #[serde(default)]
    context_name: Option<String>,
    #[serde(default)]
    plannable_type: String,
    #[serde(default)]
    plannable_date: Option<String>,
    #[serde(default)]
    html_url: Option<String>,
    #[serde(default, deserialize_with = "null_default")]
    plannable: Plannable,
    /// 有提交的条目是对象，日程、笔记等是 false
    #[serde(default)]
    submissions: Value,
    #[serde(default, deserialize_with = "null_default")]
    planner_override: PlannerOverride,
}

#[derive(serde::Deserialize, Default)]
struct Plannable {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    due_at: Option<String>,
    #[serde(default)]
    todo_date: Option<String>,
    #[serde(default)]
    start_at: Option<String>,
    #[serde(default)]
    points_possible: Option<f64>,
    #[serde(default)]
    location_name: Option<String>,
}

#[derive(serde::Deserialize, Default)]
struct PlannerOverride {
    #[serde(default, deserialize_with = "null_default")]
    marked_complete: bool,
    #[serde(default, deserialize_with = "null_default")]
    dismissed: bool,
}

#[derive(PartialEq, Eq)]
enum State {
    Done,
    Pending,
    /// 日程、笔记这类没有提交的条目
    NoSubmission,
}

impl PlannerItem {
    fn time(&self) -> Option<DateTime<FixedOffset>> {
        [
            &self.plannable.due_at,
            &self.plannable.todo_date,
            &self.plannable.start_at,
            &self.plannable_date,
        ]
        .into_iter()
        .flatten()
        .find_map(|text| parse_time(text))
    }

    fn kind(&self) -> &str {
        match self.plannable_type.as_str() {
            "assignment" => "作业",
            "quiz" => "测验",
            "discussion_topic" => "讨论",
            "calendar_event" => "日程",
            "planner_note" => "个人待办",
            "wiki_page" => "页面",
            "assessment_request" => "互评",
            other => other,
        }
    }

    fn flag(&self, key: &str) -> bool {
        self.submissions[key].as_bool() == Some(true)
    }

    fn state(&self) -> State {
        if self.planner_override.marked_complete || self.flag("submitted") || self.flag("excused") || self.flag("graded") {
            State::Done
        } else if self.submissions.is_object() || self.plannable_type == "planner_note" {
            State::Pending
        } else {
            State::NoSubmission
        }
    }

    fn status(&self) -> String {
        if self.planner_override.marked_complete && !self.flag("submitted") {
            return "已手动标记完成".into();
        }
        let mut text = if self.flag("excused") {
            "已免交".to_string()
        } else if self.flag("graded") {
            "已批改".to_string()
        } else if self.flag("submitted") {
            "已提交".to_string()
        } else if self.flag("missing") {
            "缺交".to_string()
        } else {
            "未提交".to_string()
        };
        if self.flag("late") {
            text.push_str("，迟交");
        }
        if self.flag("has_feedback") {
            text.push_str("，有老师反馈");
        }
        if self.flag("redo_request") {
            text.push_str("，老师要求重交");
        }
        text
    }

    fn headline(&self, now: DateTime<FixedOffset>) -> String {
        let when = self.time().map_or("时间未知".to_string(), |time| fmt_day_time(time, now));
        format!(
            "{when} · {} · {} · {}",
            self.context_name.as_deref().unwrap_or("个人"),
            self.kind(),
            self.plannable.title.as_deref().unwrap_or("(无标题)").trim()
        )
    }
}

pub(super) fn render_deadlines(
    items: &[PlannerItem],
    canvas: &Canvas,
    now: DateTime<FixedOffset>,
    days: i64,
    include_completed: bool,
) -> String {
    let mut upcoming = Vec::new();
    let mut overdue = Vec::new();
    let mut done = Vec::new();
    for item in items.iter().filter(|item| !item.planner_override.dismissed && item.plannable_type != "announcement") {
        let Some(time) = item.time() else { continue };
        match item.state() {
            State::Done => done.push((time, item)),
            _ if time >= now => upcoming.push((time, item)),
            State::Pending => overdue.push((time, item)),
            State::NoSubmission => {}
        }
    }
    upcoming.sort_by_key(|(time, _)| *time);
    overdue.sort_by_key(|(time, _)| std::cmp::Reverse(*time));
    done.sort_by_key(|(time, _)| *time);

    let week = campus_time::week_of(now.date_naive())
        .map(|info| format!("，第 {} 周", info.week))
        .unwrap_or_default();
    let mut text = format!(
        "未来 {days} 天的截止事项（北京时间，当前 {}{week}）\n",
        campus_time::fmt_time(now, now)
    );
    let pending = upcoming.iter().filter(|(_, item)| item.state() == State::Pending).count();
    text.push_str(&format!(
        "待完成 {pending} 项，日程 {} 项，最近 {OVERDUE_DAYS} 天逾期未交 {} 项，窗口内已完成 {} 项\n",
        upcoming.len() - pending,
        overdue.len(),
        done.len()
    ));

    if upcoming.is_empty() {
        text.push_str("\n接下来没有截止事项。\n");
    } else {
        text.push_str("\n—— 即将截止 ——\n");
        for (index, (time, item)) in upcoming.iter().enumerate() {
            text.push_str(&format!("{}. {}\n", index + 1, item.headline(now)));
            let mut facts = vec![fmt_remaining(*time, now)];
            if item.state() == State::Pending {
                facts.push(item.status());
            }
            if let Some(points) = item.plannable.points_possible.filter(|points| *points > 0.0) {
                facts.push(format!("{} 分", trim_number(points)));
            }
            if let Some(location) = item.plannable.location_name.as_deref().filter(|place| !place.trim().is_empty()) {
                facts.push(format!("地点 {location}"));
            }
            text.push_str(&format!("   {}\n", facts.join(" · ")));
            if let Some(url) = &item.html_url {
                text.push_str(&format!("   {}\n", canvas.url(url)));
            }
        }
    }
    if !overdue.is_empty() {
        text.push_str("\n—— 已截止但还没交 ——\n");
        for (_, item) in &overdue {
            text.push_str(&format!("- {}（{}）\n", item.headline(now), item.status()));
        }
        text.push_str("可能还能补交，可用 get_exam 查看该课程作业的最晚提交时间。\n");
    }
    if include_completed && !done.is_empty() {
        text.push_str("\n—— 已完成 ——\n");
        for (_, item) in &done {
            text.push_str(&format!("- {}（{}）\n", item.headline(now), item.status()));
        }
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

pub(super) async fn get_deadlines(arguments: Value) -> Result<String, String> {
    let days = window_days(&arguments)?;
    let include_completed = arguments["include_completed"].as_bool().unwrap_or(false);
    let canvas = Canvas::from_env()?;
    let now = campus_time::now();
    let items: Vec<PlannerItem> = canvas
        .get_all(
            "/api/v1/planner/items",
            &[
                ("start_date", (now - Duration::days(OVERDUE_DAYS)).to_rfc3339()),
                ("end_date", (now + Duration::days(days)).to_rfc3339()),
            ],
            "计划器（近期截止事项）",
        )
        .await?;
    Ok(clip(&render_deadlines(&items, &canvas, now, days, include_completed)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use serde_json::json;

    #[test]
    fn deadlines_split_into_upcoming_overdue_and_done() {
        let now = campus_time::beijing().with_ymd_and_hms(2026, 11, 25, 12, 0, 0).unwrap();
        let items: Vec<PlannerItem> = serde_json::from_value(json!([
            {"context_name": "高等数学", "plannable_type": "assignment", "plannable_date": "2026-11-26T15:59:59Z",
             "html_url": "/courses/1/assignments/11",
             "plannable": {"title": "习题 5", "due_at": "2026-11-26T15:59:59Z", "points_possible": 10.0},
             "submissions": {"submitted": false, "missing": false, "late": false, "graded": false, "excused": false},
             "planner_override": null},
            {"context_name": "大学物理", "plannable_type": "quiz", "plannable_date": "2026-11-25T14:00:00Z",
             "plannable": {"title": "小测 3", "due_at": "2026-11-25T14:00:00Z"},
             "submissions": {"submitted": true, "graded": false}},
            {"context_name": "线性代数", "plannable_type": "assignment", "plannable_date": "2026-11-20T15:59:59Z",
             "plannable": {"title": "HW4", "due_at": "2026-11-20T15:59:59Z"},
             "submissions": {"submitted": false, "missing": true}},
            {"context_name": "线性代数", "plannable_type": "calendar_event", "plannable_date": "2026-11-30T02:00:00Z",
             "plannable": {"title": "期中考试", "start_at": "2026-11-30T02:00:00Z", "location_name": "东上院 102"},
             "submissions": false},
            {"context_name": "思政", "plannable_type": "announcement", "plannable_date": "2026-11-26T00:00:00Z",
             "plannable": {"title": "通知"}, "submissions": false},
            {"context_name": "体育", "plannable_type": "assignment", "plannable_date": "2026-11-27T15:59:59Z",
             "plannable": {"title": "打卡", "due_at": "2026-11-27T15:59:59Z"},
             "submissions": {"submitted": false}, "planner_override": {"marked_complete": true, "dismissed": false}},
            {"context_name": "旧课", "plannable_type": "assignment", "plannable_date": "2026-11-28T15:59:59Z",
             "plannable": {"title": "隐藏", "due_at": "2026-11-28T15:59:59Z"},
             "submissions": {"submitted": false}, "planner_override": {"marked_complete": false, "dismissed": true}}
        ]))
        .unwrap();
        let canvas = Canvas::new(super::super::canvas::CANVAS_BASE, "token".into());
        let text = render_deadlines(&items, &canvas, now, 14, false);
        assert!(text.contains("待完成 1 项，日程 1 项，最近 14 天逾期未交 1 项，窗口内已完成 2 项"), "{text}");
        assert!(text.contains("1. 明天 周四 23:59 · 高等数学 · 作业 · 习题 5"), "{text}");
        assert!(text.contains("还剩 1 天 11 小时 · 未提交 · 10 分"), "{text}");
        assert!(text.contains("https://oc.sjtu.edu.cn/courses/1/assignments/11"));
        assert!(text.contains("2. 11月30日 周一 10:00 · 线性代数 · 日程 · 期中考试"), "{text}");
        assert!(text.contains("地点 东上院 102"));
        assert!(text.contains("- 11月20日 周五 23:59 · 线性代数 · 作业 · HW4（缺交）"), "{text}");
        assert!(!text.contains("通知") && !text.contains("隐藏") && !text.contains("—— 已完成"), "{text}");

        let with_done = render_deadlines(&items, &canvas, now, 14, true);
        assert!(with_done.contains("今天 周三 22:00 · 大学物理 · 测验 · 小测 3（已提交）"), "{with_done}");
        assert!(with_done.contains("体育 · 作业 · 打卡（已手动标记完成）"), "{with_done}");
    }

    #[test]
    fn window_is_validated() {
        assert_eq!(window_days(&json!({})).unwrap(), 14);
        assert_eq!(window_days(&json!({"days": null})).unwrap(), 14);
        assert_eq!(window_days(&json!({"days": 7})).unwrap(), 7);
        assert!(window_days(&json!({"days": 0})).is_err());
        assert!(window_days(&json!({"days": 365})).is_err());
    }
}
