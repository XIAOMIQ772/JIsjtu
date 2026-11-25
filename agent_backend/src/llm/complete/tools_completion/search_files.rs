//! search_files：在所有进行中课程的 Canvas 文件里按文件名搜索，不用先知道是哪门课。

use futures_util::future::join_all;
use serde_json::Value;

use super::campus_time::{self, parse_time};
use super::canvas::Canvas;
use super::clip;
use super::course_files::{CanvasFile, fetch_course_files};

const MAX_RESULTS: usize = 40;

#[derive(serde::Deserialize)]
struct Course {
    id: u64,
    #[serde(default)]
    name: String,
}

/// 关键词按空格拆开，全部出现在文件名里才算匹配（不区分大小写）。
pub(super) fn matches(name: &str, keywords: &[String]) -> bool {
    let name = name.to_lowercase();
    keywords.iter().all(|keyword| name.contains(keyword.as_str()))
}

pub(super) fn render(
    keyword: &str,
    found: &[(String, u64, &CanvasFile)],
    searched: usize,
    unavailable: &[String],
    now: chrono::DateTime<chrono::FixedOffset>,
) -> String {
    let mut text = if found.is_empty() {
        format!("在 {searched} 门课程的文件里没有找到名称包含「{keyword}」的文件。可以换个关键词，或用 get_announcements / get_exam 看看是否在公告或作业附件里。\n")
    } else {
        format!(
            "在 {searched} 门课程的文件里找到 {} 个名称包含「{keyword}」的文件{}：\n",
            found.len(),
            if found.len() > MAX_RESULTS { format!("（显示最新的 {MAX_RESULTS} 个）") } else { String::new() }
        )
    };
    for (course, course_id, file) in found.iter().take(MAX_RESULTS) {
        let updated = file
            .updated_at
            .as_deref()
            .and_then(parse_time)
            .map(|time| format!(" · 更新于 {}", campus_time::fmt_time(time, now).split(' ').next().unwrap_or_default()))
            .unwrap_or_default();
        text.push_str(&format!(
            "- 【{course}】{}（{}{updated}）· 课程 id {course_id} · 文件 id {}\n",
            file.display_name.trim(),
            file.human_size(),
            file.id
        ));
    }
    if !found.is_empty() {
        text.push_str("下载用 course_files（action=download，传课程 id 和文件 id），再用 pdf_analysis 或 read_office 阅读。\n");
    }
    if !unavailable.is_empty() {
        text.push_str(&format!("以下课程的文件页未开放或读取失败，没有搜索：{}\n", unavailable.join("、")));
    }
    text
}

pub(super) async fn search_files(arguments: Value) -> Result<String, String> {
    let keyword = arguments["keyword"]
        .as_str()
        .map(str::trim)
        .filter(|keyword| !keyword.is_empty())
        .ok_or("请提供要搜索的文件名关键词，例如“实验指导书”“期中”“chapter 3”")?;
    let keywords: Vec<String> = keyword.split_whitespace().map(str::to_lowercase).collect();
    let canvas = Canvas::from_env()?;
    let courses: Vec<Course> = canvas
        .get_all("/api/v1/courses", &[("enrollment_state", "active".into())], "课程列表")
        .await?;
    let token = std::env::var("CANVAS_API_TOKEN").unwrap_or_default();
    let client = reqwest::Client::new();
    let listings = join_all(courses.iter().map(|course| {
        let (client, token) = (&client, token.trim());
        async move { (course, fetch_course_files(client, token, &course.id.to_string()).await) }
    }))
    .await;

    let mut unavailable = Vec::new();
    let mut found: Vec<(String, u64, &CanvasFile)> = Vec::new();
    for (course, result) in &listings {
        match result {
            Ok(files) => found.extend(
                files
                    .iter()
                    .filter(|file| matches(&file.display_name, &keywords))
                    .map(|file| (course.name.trim().to_string(), course.id, file)),
            ),
            Err(_) => unavailable.push(course.name.trim().to_string()),
        }
    }
    // 最近更新的排在前面
    found.sort_by_key(|(_, _, file)| std::cmp::Reverse(file.updated_at.as_deref().and_then(parse_time)));
    Ok(clip(&render(keyword, &found, courses.len(), &unavailable, campus_time::now())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use serde_json::json;

    #[test]
    fn keywords_must_all_match_and_results_name_their_course() {
        let keywords = vec!["实验".to_string(), "pdf".to_string()];
        assert!(matches("物理实验指导书.PDF", &keywords));
        assert!(!matches("物理实验数据.xlsx", &keywords));
        let file: CanvasFile = serde_json::from_value(json!({
            "id": 42, "display_name": "物理实验指导书.pdf", "size": 2097152, "updated_at": "2026-11-20T02:00:00Z"
        }))
        .unwrap();
        let now = campus_time::beijing().with_ymd_and_hms(2026, 11, 25, 12, 0, 0).unwrap();
        let text = render("实验 pdf", &[("大学物理实验（2）".into(), 93172, &file)], 9, &["思政".into()], now);
        assert!(text.contains("在 9 门课程的文件里找到 1 个名称包含「实验 pdf」的文件："), "{text}");
        assert!(text.contains("- 【大学物理实验（2）】物理实验指导书.pdf（2.0 MB · 更新于 11月20日）· 课程 id 93172 · 文件 id 42"), "{text}");
        assert!(text.contains("没有搜索：思政"), "{text}");
        assert!(render("无", &[], 9, &[], now).contains("没有找到"));
    }
}
