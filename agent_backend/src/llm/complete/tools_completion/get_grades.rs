//! get_grades：Canvas 成绩。不传课程时给各课程总评概览和最近批改，传课程时给逐项明细、
//! 班级统计、分组权重与老师评语。

use std::collections::HashMap;

use chrono::{DateTime, Duration, FixedOffset};
use futures_util::future::join_all;
use serde_json::Value;

use super::campus_time::{self, fmt_day_time, parse_time};
use super::canvas::{Canvas, Submission, null_default, trim_number};
use super::clip;
use super::text::{one_line, shorten};

const RECENT_DAYS: i64 = 30;
const COMMENT_CHARS: usize = 200;

#[derive(serde::Deserialize)]
pub(super) struct CourseInfo {
    id: u64,
    #[serde(default)]
    name: String,
    #[serde(default, deserialize_with = "null_default")]
    apply_assignment_group_weights: bool,
    #[serde(default, deserialize_with = "null_default")]
    enrollments: Vec<Enrollment>,
}

#[derive(serde::Deserialize)]
struct Enrollment {
    #[serde(default, rename = "type")]
    kind: Option<String>,
    #[serde(default)]
    computed_current_score: Option<f64>,
    #[serde(default)]
    computed_current_grade: Option<String>,
}

impl CourseInfo {
    /// 学生身份的总评，“87.5（B+）”；老师隐藏总评时为 None。
    fn total(&self) -> Option<String> {
        let enrollment = self
            .enrollments
            .iter()
            .find(|enrollment| enrollment.kind.as_deref().is_none_or(|kind| kind == "student"))?;
        let score = enrollment.computed_current_score?;
        Some(match enrollment.computed_current_grade.as_deref().filter(|grade| !grade.is_empty()) {
            Some(grade) => format!("{}（{grade}）", trim_number(score)),
            None => trim_number(score),
        })
    }
}

#[derive(serde::Deserialize)]
pub(super) struct Group {
    #[serde(default)]
    name: String,
    #[serde(default)]
    group_weight: Option<f64>,
    #[serde(default)]
    rules: Value,
    #[serde(default, deserialize_with = "null_default")]
    assignments: Vec<GradedAssignment>,
}

#[derive(serde::Deserialize)]
struct GradedAssignment {
    id: u64,
    #[serde(default)]
    name: String,
    #[serde(default)]
    points_possible: Option<f64>,
    #[serde(default)]
    due_at: Option<String>,
    #[serde(default)]
    position: Option<i64>,
    #[serde(default, deserialize_with = "null_default")]
    omit_from_final_grade: bool,
    #[serde(default, deserialize_with = "null_default")]
    submission_types: Vec<String>,
    #[serde(default)]
    submission: Option<Submission>,
    #[serde(default)]
    score_statistics: Option<Stats>,
}

#[derive(serde::Deserialize)]
struct Stats {
    #[serde(default)]
    min: Option<f64>,
    #[serde(default)]
    max: Option<f64>,
    #[serde(default)]
    mean: Option<f64>,
}

#[derive(serde::Deserialize)]
pub(super) struct CommentedSubmission {
    #[serde(default)]
    assignment_id: u64,
    #[serde(default)]
    user_id: Option<u64>,
    #[serde(default)]
    score: Option<f64>,
    #[serde(default)]
    grade: Option<String>,
    #[serde(default)]
    graded_at: Option<String>,
    #[serde(default)]
    assignment: Option<AssignmentRef>,
    #[serde(default, deserialize_with = "null_default")]
    submission_comments: Vec<Comment>,
}

#[derive(serde::Deserialize)]
pub(super) struct AssignmentRef {
    #[serde(default)]
    name: String,
    #[serde(default)]
    points_possible: Option<f64>,
}

#[derive(serde::Deserialize)]
struct Comment {
    #[serde(default)]
    author_id: Option<u64>,
    #[serde(default)]
    author_name: Option<String>,
    #[serde(default)]
    comment: Option<String>,
}

impl CommentedSubmission {
    /// 老师、助教留下的评语（不含学生自己的留言）。
    fn feedback(&self) -> Vec<String> {
        self.submission_comments
            .iter()
            .filter(|comment| comment.author_id.is_none() || comment.author_id != self.user_id)
            .filter_map(|comment| {
                let text = one_line(comment.comment.as_deref()?);
                (!text.is_empty()).then(|| {
                    format!(
                        "{}：{}",
                        comment.author_name.as_deref().unwrap_or("老师"),
                        shorten(&text, COMMENT_CHARS)
                    )
                })
            })
            .collect()
    }
}

fn score_text(score: f64, points: Option<f64>) -> String {
    match points.filter(|points| *points > 0.0) {
        Some(points) => format!("{}/{}", trim_number(score), trim_number(points)),
        None => trim_number(score),
    }
}

fn rules_text(rules: &Value) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(n) = rules["drop_lowest"].as_u64().filter(|n| *n > 0) {
        parts.push(format!("去掉最低 {n} 次"));
    }
    if let Some(n) = rules["drop_highest"].as_u64().filter(|n| *n > 0) {
        parts.push(format!("去掉最高 {n} 次"));
    }
    (!parts.is_empty()).then(|| parts.join("、"))
}

pub(super) fn render_course(
    course: &CourseInfo,
    groups: &mut [Group],
    comments: &[CommentedSubmission],
    now: DateTime<FixedOffset>,
) -> String {
    let feedback: HashMap<u64, Vec<String>> = comments
        .iter()
        .map(|submission| (submission.assignment_id, submission.feedback()))
        .filter(|(_, feedback)| !feedback.is_empty())
        .collect();
    let all: Vec<&GradedAssignment> = groups.iter().flat_map(|group| &group.assignments).collect();
    let graded_count = all
        .iter()
        .filter(|assignment| assignment.submission.as_ref().is_some_and(|s| s.score.is_some()))
        .count();
    let mut text = format!("《{}》成绩明细（Canvas）\n", course.name.trim());
    match course.total() {
        Some(total) => text.push_str(&format!("当前总评：{total}；已批改 {graded_count}/{} 项\n", all.len())),
        None => text.push_str(&format!("老师未公开总评；已批改 {graded_count}/{} 项\n", all.len())),
    }
    let weighted = course.apply_assignment_group_weights;
    if weighted {
        let weights: Vec<String> = groups
            .iter()
            .map(|group| format!("{} {}%", group.name, trim_number(group.group_weight.unwrap_or_default())))
            .collect();
        text.push_str(&format!("分组权重：{}\n", weights.join("、")));
    }

    // 按已批改部分估算的加权得分率，只在 Canvas 没给总评时作参考
    let mut weighted_sum = 0.0;
    let mut weight_total = 0.0;
    for group in groups.iter_mut() {
        group.assignments.sort_by(|a, b| {
            (a.due_at.as_deref().and_then(parse_time), a.position).cmp(&(b.due_at.as_deref().and_then(parse_time), b.position))
        });
        let mut earned = 0.0;
        let mut possible = 0.0;
        let mut graded = 0;
        for assignment in &group.assignments {
            if let (Some(score), Some(points)) = (
                assignment.submission.as_ref().and_then(|s| s.score),
                assignment.points_possible.filter(|points| *points > 0.0),
            ) {
                graded += 1;
                if !assignment.omit_from_final_grade {
                    earned += score;
                    possible += points;
                }
            }
        }
        let mut title = format!("\n—— {}", group.name.trim());
        if weighted {
            title.push_str(&format!(" · 权重 {}%", trim_number(group.group_weight.unwrap_or_default())));
        }
        title.push_str(&format!(" · 已批改 {graded}/{} 项", group.assignments.len()));
        if possible > 0.0 {
            title.push_str(&format!(
                " · 得分 {}/{}（{:.1}%）",
                trim_number(earned),
                trim_number(possible),
                earned / possible * 100.0
            ));
            if weighted {
                weighted_sum += group.group_weight.unwrap_or_default() * earned / possible;
                weight_total += group.group_weight.unwrap_or_default();
            }
        }
        if let Some(rules) = rules_text(&group.rules) {
            title.push_str(&format!(" · 规则：{rules}"));
        }
        text.push_str(&title);
        text.push_str(" ——\n");
        for assignment in &group.assignments {
            let submission = assignment.submission.clone().unwrap_or_default();
            let mut line = format!("- {}：", assignment.name.trim());
            match submission.score {
                Some(score) => {
                    line.push_str(&score_text(score, assignment.points_possible));
                    if let Some(grade) = submission.grade.as_deref().filter(|grade| grade.parse::<f64>().is_err()) {
                        line.push_str(&format!("（{grade}）"));
                    }
                    if let Some(stats) = &assignment.score_statistics {
                        let mut parts = Vec::new();
                        if let Some(mean) = stats.mean {
                            parts.push(format!("班均 {}", trim_number((mean * 10.0).round() / 10.0)));
                        }
                        if let Some(max) = stats.max {
                            parts.push(format!("最高 {}", trim_number(max)));
                        }
                        if let Some(min) = stats.min {
                            parts.push(format!("最低 {}", trim_number(min)));
                        }
                        if !parts.is_empty() {
                            line.push_str(&format!(" · {}", parts.join("，")));
                        }
                    }
                    if let Some(time) = submission.graded_at.as_deref().and_then(parse_time) {
                        line.push_str(&format!(" · {}批改", campus_time::fmt_time(time, now).split(' ').next().unwrap_or_default()));
                    }
                }
                None if !assignment
                    .submission_types
                    .iter()
                    .any(|kind| !matches!(kind.as_str(), "none" | "on_paper" | "not_graded")) =>
                {
                    line.push_str("未出分");
                }
                None => {
                    let due_passed = assignment
                        .due_at
                        .as_deref()
                        .and_then(parse_time)
                        .is_some_and(|due| due < now);
                    line.push_str(&submission.status(assignment.points_possible, due_passed, now));
                }
            }
            if assignment.omit_from_final_grade {
                line.push_str(" · 不计入总评");
            }
            text.push_str(&line);
            text.push('\n');
            for comment in feedback.get(&assignment.id).into_iter().flatten() {
                text.push_str(&format!("  评语（{comment}）\n"));
            }
        }
    }
    if weighted && course.total().is_none() && weight_total > 0.0 {
        text.push_str(&format!(
            "\n按已批改部分估算的加权得分率：{:.1}%（仅供参考，以老师公布为准）\n",
            weighted_sum / weight_total * 100.0
        ));
    }
    text
}

pub(super) fn render_overview(
    courses: &[CourseInfo],
    graded: &[(String, Result<Vec<CommentedSubmission>, String>)],
    now: DateTime<FixedOffset>,
) -> String {
    let mut text = String::from("各课程成绩概览（Canvas）\n");
    let mut recent: Vec<(DateTime<FixedOffset>, &str, &CommentedSubmission)> = Vec::new();
    for (course, (_, result)) in courses.iter().zip(graded) {
        let mut line = format!("- {}：", course.name.trim());
        line.push_str(&match course.total() {
            Some(total) => format!("当前总评 {total}"),
            None => "老师未公开总评".to_string(),
        });
        match result {
            Ok(submissions) => {
                line.push_str(&format!("，已批改 {} 项", submissions.len()));
                for submission in submissions {
                    if let Some(time) = submission.graded_at.as_deref().and_then(parse_time) {
                        if now - time <= Duration::days(RECENT_DAYS) {
                            recent.push((time, course.name.trim(), submission));
                        }
                    }
                }
            }
            Err(error) => line.push_str(&format!("（批改记录读取失败：{error}）")),
        }
        text.push_str(&line);
        text.push('\n');
    }
    recent.sort_by_key(|(time, _, _)| std::cmp::Reverse(*time));
    if recent.is_empty() {
        text.push_str(&format!("\n最近 {RECENT_DAYS} 天没有新批改的作业。\n"));
    } else {
        text.push_str(&format!("\n最近 {RECENT_DAYS} 天新批改（{} 项，按批改时间从新到旧）：\n", recent.len()));
        for (time, course, submission) in recent.iter().take(20) {
            let assignment = submission.assignment.as_ref();
            let name = assignment.map_or("(未知作业)", |assignment| assignment.name.trim());
            let mut line = format!("- {} · {course} · {name}", fmt_day_time(*time, now));
            if let Some(score) = submission.score {
                line.push_str(&format!("：{}", score_text(score, assignment.and_then(|a| a.points_possible))));
                if let Some(grade) = submission.grade.as_deref().filter(|grade| grade.parse::<f64>().is_err()) {
                    line.push_str(&format!("（{grade}）"));
                }
            }
            let feedback = submission.feedback();
            if !feedback.is_empty() {
                line.push_str(&format!(" · 有 {} 条评语", feedback.len()));
            }
            text.push_str(&line);
            text.push('\n');
        }
    }
    text.push_str("\n要看某门课的逐项分数、班级均分、权重和评语，请传 course_id。");
    text
}

pub(super) async fn get_grades(arguments: Value) -> Result<String, String> {
    let course_id = match &arguments["course_id"] {
        Value::String(id) if !id.trim().is_empty() => Some(id.trim().to_string()),
        Value::Number(id) => Some(id.to_string()),
        _ => None,
    };
    if course_id.as_ref().is_some_and(|id| !id.chars().all(|c| c.is_ascii_digit())) {
        return Err("course_id 应为数字，可先用 get_courses 查询".into());
    }
    let canvas = Canvas::from_env()?;
    let now = campus_time::now();

    if let Some(id) = course_id {
        let course_path = format!("/api/v1/courses/{id}");
        let course_query = [("include[]", "total_scores".to_string())];
        let groups_path = format!("/api/v1/courses/{id}/assignment_groups");
        let groups_query = [
            ("include[]", "assignments".to_string()),
            ("include[]", "submission".to_string()),
            ("include[]", "score_statistics".to_string()),
        ];
        let comments_path = format!("/api/v1/courses/{id}/students/submissions");
        let comments_query = [
            ("student_ids[]", "self".to_string()),
            ("include[]", "submission_comments".to_string()),
        ];
        let (course, mut groups, comments) = tokio::try_join!(
            canvas.get::<CourseInfo>(&course_path, &course_query, "课程信息"),
            canvas.get_all::<Group>(&groups_path, &groups_query, "作业分组与成绩"),
            canvas.get_all::<CommentedSubmission>(&comments_path, &comments_query, "提交评语"),
        )?;
        return Ok(clip(&render_course(&course, &mut groups, &comments, now)));
    }

    let courses: Vec<CourseInfo> = canvas
        .get_all(
            "/api/v1/courses",
            &[("enrollment_state", "active".into()), ("include[]", "total_scores".into())],
            "课程列表",
        )
        .await?;
    let graded = join_all(courses.iter().map(|course| {
        let canvas = &canvas;
        async move {
            let result = canvas
                .get_all::<CommentedSubmission>(
                    &format!("/api/v1/courses/{}/students/submissions", course.id),
                    &[
                        ("student_ids[]", "self".into()),
                        ("workflow_state", "graded".into()),
                        ("include[]", "assignment".into()),
                        ("include[]", "submission_comments".into()),
                    ],
                    "批改记录",
                )
                .await;
            (course.name.clone(), result)
        }
    }))
    .await;
    Ok(clip(&render_overview(&courses, &graded, now)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use serde_json::json;

    fn now() -> DateTime<FixedOffset> {
        campus_time::beijing().with_ymd_and_hms(2026, 11, 25, 12, 0, 0).unwrap()
    }

    #[test]
    fn course_detail_shows_scores_statistics_weights_and_teacher_feedback_only() {
        let course: CourseInfo = serde_json::from_value(json!({
            "id": 1, "name": "大学物理", "apply_assignment_group_weights": true,
            "enrollments": [{"type": "student", "computed_current_score": null, "computed_current_grade": null}]
        }))
        .unwrap();
        let mut groups: Vec<Group> = serde_json::from_value(json!([
            {"name": "作业", "group_weight": 40.0, "rules": {"drop_lowest": 1},
             "assignments": [
                {"id": 11, "name": "HW2", "points_possible": 10.0, "due_at": "2026-11-10T15:59:00Z", "submission_types": ["online_upload"],
                 "submission": {"workflow_state": "graded", "score": 8.0, "graded_at": "2026-11-12T02:00:00Z"},
                 "score_statistics": {"min": 3.0, "max": 10.0, "mean": 7.25}},
                {"id": 10, "name": "HW1", "points_possible": 10.0, "due_at": "2026-11-03T15:59:00Z", "submission_types": ["online_upload"],
                 "submission": {"workflow_state": "graded", "score": 10.0, "grade": "10"}},
                {"id": 12, "name": "HW3", "points_possible": 10.0, "due_at": "2026-12-01T15:59:00Z", "submission_types": ["online_upload"],
                 "submission": {"workflow_state": "unsubmitted"}}
             ]},
            {"name": "期末", "group_weight": 60.0, "rules": {}, "assignments": [
                {"id": 20, "name": "期末考试", "points_possible": 100.0, "submission_types": ["on_paper"], "submission": null}
            ]}
        ]))
        .unwrap();
        let comments: Vec<CommentedSubmission> = serde_json::from_value(json!([
            {"assignment_id": 11, "user_id": 7, "submission_comments": [
                {"author_id": 99, "author_name": "张助教", "comment": "第 2 题\n单位写错"},
                {"author_id": 7, "author_name": "我", "comment": "老师我想申诉"}
            ]}
        ]))
        .unwrap();
        let text = render_course(&course, &mut groups, &comments, now());
        assert!(text.contains("老师未公开总评；已批改 2/4 项"), "{text}");
        assert!(text.contains("分组权重：作业 40%、期末 60%"), "{text}");
        assert!(text.contains("—— 作业 · 权重 40% · 已批改 2/3 项 · 得分 18/20（90.0%） · 规则：去掉最低 1 次 ——"), "{text}");
        assert!(text.find("- HW1：10/10").unwrap() < text.find("- HW2：8/10").unwrap(), "按截止时间排序：{text}");
        assert!(text.contains("- HW2：8/10 · 班均 7.3，最高 10，最低 3 · 11月12日批改"), "{text}");
        assert!(text.contains("  评语（张助教：第 2 题 单位写错）"), "{text}");
        assert!(!text.contains("申诉"), "学生自己的留言不是评语：{text}");
        assert!(text.contains("- HW3：未提交"), "{text}");
        assert!(text.contains("- 期末考试：未出分"), "{text}");
        assert!(text.contains("按已批改部分估算的加权得分率：90.0%"), "{text}");
    }

    #[test]
    fn overview_lists_totals_and_recent_grades() {
        let courses: Vec<CourseInfo> = serde_json::from_value(json!([
            {"id": 1, "name": "线性代数", "enrollments": [{"type": "student", "computed_current_score": 92.5, "computed_current_grade": "A"}]},
            {"id": 2, "name": "思政", "enrollments": [{"type": "student"}]}
        ]))
        .unwrap();
        let recent: Vec<CommentedSubmission> = serde_json::from_value(json!([
            {"assignment_id": 1, "user_id": 7, "score": 9.0, "graded_at": "2026-11-24T08:00:00Z",
             "assignment": {"name": "HW5", "points_possible": 10.0},
             "submission_comments": [{"author_id": 1, "author_name": "李老师", "comment": "好"}]},
            {"assignment_id": 2, "user_id": 7, "score": 5.0, "graded_at": "2026-09-01T08:00:00Z",
             "assignment": {"name": "HW1", "points_possible": 10.0}}
        ]))
        .unwrap();
        let graded = vec![
            ("线性代数".to_string(), Ok(recent)),
            ("思政".to_string(), Err("Canvas 拒绝访问".to_string())),
        ];
        let text = render_overview(&courses, &graded, now());
        assert!(text.contains("- 线性代数：当前总评 92.5（A），已批改 2 项"), "{text}");
        assert!(text.contains("- 思政：老师未公开总评（批改记录读取失败：Canvas 拒绝访问）"), "{text}");
        assert!(text.contains("- 昨天 周二 16:00 · 线性代数 · HW5：9/10 · 有 1 条评语"), "{text}");
        assert!(!text.contains("HW1"), "30 天前的批改不算最近：{text}");
    }
}
