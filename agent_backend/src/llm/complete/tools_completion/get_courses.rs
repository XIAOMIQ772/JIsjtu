//! get_courses：Canvas 上进行中的课程，附课程代码、学期和授课老师。

use super::canvas::{Canvas, null_default};
use super::clip;

#[derive(serde::Deserialize)]
pub(super) struct Course {
    id: u64,
    #[serde(default)]
    name: String,
    #[serde(default)]
    course_code: Option<String>,
    #[serde(default)]
    term: Option<Term>,
    #[serde(default, deserialize_with = "null_default")]
    teachers: Vec<Teacher>,
}

#[derive(serde::Deserialize)]
struct Term {
    #[serde(default)]
    name: Option<String>,
}

#[derive(serde::Deserialize)]
struct Teacher {
    #[serde(default)]
    display_name: Option<String>,
}

static COURSE_NUMBER: std::sync::LazyLock<regex::Regex> =
    std::sync::LazyLock::new(|| regex::Regex::new(r"\b([A-Z]{2,6}\d{3,5})(?:-(\d{1,3}))?\b").unwrap());

/// 交大的 course_code 形如“本-(2026-2027-1)-PHY1262-07-大学物理（A类）（2）”，只保留课号和班号。
fn short_code(code: &str, name: &str) -> Option<String> {
    let code = code.trim();
    if code.is_empty() || code == name.trim() {
        return None;
    }
    match COURSE_NUMBER.captures(code) {
        Some(caps) => Some(match caps.get(2) {
            Some(class) => format!("课号 {}（{} 班）", &caps[1], class.as_str()),
            None => format!("课号 {}", &caps[1]),
        }),
        None => Some(code.to_string()),
    }
}

pub(super) fn render_courses(courses: &[Course]) -> String {
    if courses.is_empty() {
        return "没有进行中的课程。".to_string();
    }
    let mut text = format!("共 {} 门课程：\n", courses.len());
    for course in courses {
        text.push_str(&format!("  [{:>7}]  {}\n", course.id, course.name.trim()));
        let mut facts = Vec::new();
        if let Some(code) = course.course_code.as_deref().and_then(|code| short_code(code, &course.name)) {
            facts.push(code);
        }
        if let Some(term) = course.term.as_ref().and_then(|term| term.name.as_deref()) {
            facts.push(term.to_string());
        }
        let teachers: Vec<&str> = course
            .teachers
            .iter()
            .filter_map(|teacher| teacher.display_name.as_deref())
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .collect();
        if !teachers.is_empty() {
            facts.push(format!("老师：{}", teachers.join("、")));
        }
        if !facts.is_empty() {
            text.push_str(&format!("            {}\n", facts.join(" · ")));
        }
    }
    text
}

pub(super) async fn get_courses(_: serde_json::Value) -> Result<String, String> {
    let canvas = Canvas::from_env()?;
    let courses: Vec<Course> = canvas
        .get_all(
            "/api/v1/courses",
            &[
                ("enrollment_state", "active".into()),
                ("include[]", "term".into()),
                ("include[]", "teachers".into()),
            ],
            "课程列表",
        )
        .await?;
    Ok(clip(&render_courses(&courses)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn courses_show_code_term_and_teachers() {
        let courses: Vec<Course> = serde_json::from_value(json!([
            {"id": 93075, "name": "概率统计", "course_code": "本-(2026-2027-1)-MATH1207-04-概率统计", "term": {"name": "2026-2027 Fall"},
             "teachers": [{"display_name": "刘老师"}, {"display_name": "王助教"}]},
            {"id": 1, "name": "形势与政策", "course_code": "形势与政策", "term": null, "teachers": null}
        ]))
        .unwrap();
        let text = render_courses(&courses);
        assert!(text.starts_with("共 2 门课程：\n  [  93075]  概率统计\n            课号 MATH1207（04 班） · 2026-2027 Fall · 老师：刘老师、王助教\n"), "{text}");
        assert!(text.ends_with("  [      1]  形势与政策\n"), "课程代码与名称相同时不重复：{text}");
        assert_eq!(render_courses(&[]), "没有进行中的课程。");
        assert_eq!(short_code("CS101", "计算机导论").as_deref(), Some("课号 CS101"));
        assert_eq!(short_code("自定义代码", "某课").as_deref(), Some("自定义代码"));
    }
}
