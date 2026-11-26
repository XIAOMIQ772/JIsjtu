use std::fmt::Write;

use super::{
    HomeworkAssignment, canvas_token, fetch_homework_assignment, homework_arg, homework_page, html_to_text,
    part_number,
};

#[derive(serde::Deserialize)]
pub(super) struct RubricCriterion {
    #[serde(flatten)]
    detail: RubricDetail,
    ratings: Option<Vec<RubricDetail>>,
    ignore_for_scoring: Option<bool>,
}

#[derive(serde::Deserialize)]
struct RubricDetail {
    description: Option<String>,
    long_description: Option<String>,
    points: Option<f64>,
}

impl RubricDetail {
    fn line(&self) -> String {
        let title = html_to_text(self.description.as_deref().unwrap_or_default());
        let body = html_to_text(self.long_description.as_deref().unwrap_or_default());
        let mut line = title.clone();
        if let Some(points) = self.points {
            let _ = write!(line, "（{points} 分）");
        }
        if !body.is_empty() && body != title {
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(&body);
        }
        line
    }
}

impl HomeworkAssignment {
    pub(super) fn requirements(&self, description: &str) -> String {
        let mut text = String::new();
        if !description.trim().is_empty() {
            let _ = writeln!(text, "\n[Canvas 作业说明]\n{description}");
        }
        if let Some(criteria) = self.rubric.as_ref().filter(|rows| !rows.is_empty()) {
            let purpose = match self.use_rubric_for_grading {
                Some(true) => "（用于评分）",
                Some(false) => "（供参考）",
                None => "",
            };
            let _ = writeln!(text, "\n[评分细则{purpose}]");
            for criterion in criteria {
                let label =
                    if criterion.ignore_for_scoring == Some(true) { "（不计入总分）" } else { "" };
                let _ = writeln!(text, "- {}{label}", criterion.detail.line());
                for rating in criterion.ratings.iter().flatten() {
                    let _ = writeln!(text, "  · {}", rating.line());
                }
            }
        }
        text
    }
}

pub(super) async fn read(args: &serde_json::Value) -> Result<String, String> {
    let (Some(course), Some(id)) = (homework_arg(args, "course_id"), homework_arg(args, "assignment_id"))
    else {
        return Err("读取老师的作业说明和评分细则需要 course_id 和 assignment_id。".into());
    };
    let part = part_number(args)?;
    let assignment = fetch_homework_assignment(&reqwest::Client::new(), &canvas_token()?, course, id).await?;
    let mut text = assignment.summary();
    let requirements =
        assignment.requirements(&html_to_text(assignment.description.as_deref().unwrap_or_default()));
    if requirements.is_empty() {
        text.push_str("Canvas 没有提供网页说明或评分细则。");
    } else {
        text.push_str(&requirements);
    }
    homework_page(&text, part)
}
