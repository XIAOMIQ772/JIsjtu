//! Canvas@SJTU（oc.sjtu.edu.cn）接口共用部分：令牌、分页、错误说明、提交状态。

use serde::de::DeserializeOwned;

use super::campus_time::{self, fmt_time};

pub(super) const CANVAS_BASE: &str = "https://oc.sjtu.edu.cn";
const MAX_PAGES: usize = 10;

pub(super) struct Canvas {
    client: reqwest::Client,
    token: String,
    base: String,
}

impl Canvas {
    pub fn from_env() -> Result<Self, String> {
        let token = std::env::var("CANVAS_API_TOKEN")
            .ok()
            .map(|token| token.trim().to_string())
            .filter(|token| !token.is_empty())
            .ok_or_else(|| {
                "未配置 CANVAS_API_TOKEN：请在 Canvas「账户 → 设置」生成访问令牌并填入 .env".to_string()
            })?;
        Ok(Self::new(CANVAS_BASE, token))
    }

    pub fn new(base: &str, token: String) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .unwrap_or_default();
        Self {
            client,
            token,
            base: base.trim_end_matches('/').to_string(),
        }
    }

    /// Canvas 返回的 html_url 有时是相对路径，统一补全成可点击的地址。
    pub fn url(&self, path_or_url: &str) -> String {
        if path_or_url.starts_with("http://") || path_or_url.starts_with("https://") {
            path_or_url.to_string()
        } else {
            format!("{}{}", self.base, path_or_url)
        }
    }

    async fn send(&self, url: &str, query: &[(&str, String)], what: &str) -> Result<reqwest::Response, String> {
        let response = self
            .client
            .get(url)
            .bearer_auth(&self.token)
            .query(query)
            .send()
            .await
            .map_err(|err| format!("请求 Canvas 失败（{what}）: {err}"))?;
        if response.status().is_success() {
            Ok(response)
        } else {
            Err(failure(response, what).await)
        }
    }

    pub async fn get<T: DeserializeOwned>(&self, path: &str, query: &[(&str, String)], what: &str) -> Result<T, String> {
        self.send(&format!("{}{path}", self.base), query, what)
            .await?
            .json()
            .await
            .map_err(|err| format!("解析 Canvas 响应失败（{what}）: {err}"))
    }

    /// 按 Link 头逐页读取列表接口，最多 MAX_PAGES 页。
    pub async fn get_all<T: DeserializeOwned>(&self, path: &str, query: &[(&str, String)], what: &str) -> Result<Vec<T>, String> {
        let mut query = query.to_vec();
        if !query.iter().any(|(key, _)| *key == "per_page") {
            query.push(("per_page", "100".into()));
        }
        let mut items = Vec::new();
        let mut response = self.send(&format!("{}{path}", self.base), &query, what).await?;
        for _ in 0..MAX_PAGES {
            let next = response
                .headers()
                .get("link")
                .and_then(|value| value.to_str().ok())
                .and_then(next_link)
                // 令牌只发给 Canvas 自己，防止 Link 头把请求引到别的主机
                .filter(|url| same_origin(url, &self.base));
            let batch: Vec<T> = response
                .json()
                .await
                .map_err(|err| format!("解析 Canvas 响应失败（{what}）: {err}"))?;
            let empty = batch.is_empty();
            items.extend(batch);
            match next {
                Some(url) if !empty => response = self.send(&url, &[], what).await?,
                _ => break,
            }
        }
        Ok(items)
    }
}

async fn failure(response: reqwest::Response, what: &str) -> String {
    let status = response.status();
    match status.as_u16() {
        401 => "Canvas 认证失败 (401)：CANVAS_API_TOKEN 无效或已过期，请重新生成".to_string(),
        403 => format!("Canvas 拒绝访问（{what}）：该课程可能未开放此页面，或你没有权限"),
        404 => format!("Canvas 中找不到{what}：课程或条目不存在，或课程未启用该页面"),
        _ => {
            let body: String = response.text().await.unwrap_or_default().chars().take(300).collect();
            format!("Canvas 返回 {status}（{what}）：{body}")
        }
    }
}

pub(super) fn next_link(header: &str) -> Option<String> {
    header.split(',').find_map(|part| {
        let mut segments = part.split(';');
        let url = segments.next()?.trim();
        segments
            .any(|segment| segment.trim() == "rel=\"next\"")
            .then(|| url.trim_start_matches('<').trim_end_matches('>').to_string())
    })
}

pub(super) fn same_origin(url: &str, base: &str) -> bool {
    match (reqwest::Url::parse(url), reqwest::Url::parse(base)) {
        (Ok(url), Ok(base)) => url.origin() == base.origin(),
        _ => false,
    }
}

/// Canvas 有时把列表或布尔字段给成 null；配合 #[serde(default)] 统一当默认值处理。
pub(super) fn null_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Default + serde::Deserialize<'de>,
{
    use serde::Deserialize;
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

/// 提交方式的中文名。
pub(super) fn submission_type_cn(kind: &str) -> &str {
    match kind {
        "online_upload" => "上传文件",
        "online_text_entry" => "在线文本",
        "online_url" => "提交网址",
        "online_quiz" => "在线测验",
        "media_recording" => "音视频",
        "student_annotation" => "在线批注",
        "discussion_topic" => "讨论区发帖",
        "external_tool" => "外部工具",
        "on_paper" => "纸质提交",
        "none" => "无需提交",
        other => other,
    }
}

/// 作业的提交情况，assignments?include[]=submission 与 students/submissions 共用。
#[derive(serde::Deserialize, Default, Clone)]
pub(super) struct Submission {
    #[serde(default)]
    pub workflow_state: Option<String>,
    #[serde(default)]
    pub submitted_at: Option<String>,
    #[serde(default)]
    pub score: Option<f64>,
    #[serde(default)]
    pub grade: Option<String>,
    #[serde(default, deserialize_with = "null_default")]
    pub late: bool,
    #[serde(default, deserialize_with = "null_default")]
    pub missing: bool,
    #[serde(default)]
    pub excused: Option<bool>,
    #[serde(default)]
    pub attempt: Option<u64>,
    #[serde(default)]
    pub graded_at: Option<String>,
}

impl Submission {
    pub fn is_submitted(&self) -> bool {
        self.submitted_at.is_some()
            || matches!(self.workflow_state.as_deref(), Some("submitted" | "graded" | "pending_review"))
    }

    pub fn is_done(&self) -> bool {
        self.is_submitted() || self.excused == Some(true)
    }

    /// “已批改 9/10”“已提交（11月20日 周五 21:03）”“缺交”等。
    pub fn status(&self, points: Option<f64>, due_passed: bool, now: chrono::DateTime<chrono::FixedOffset>) -> String {
        if self.excused == Some(true) {
            return "已免交".into();
        }
        let mut text = if self.workflow_state.as_deref() == Some("graded") && self.score.is_some() {
            let score = self.score.unwrap_or_default();
            match points {
                Some(points) if points > 0.0 => format!("已批改 {}/{}", trim_number(score), trim_number(points)),
                _ => format!("已批改 {}", trim_number(score)),
            }
        } else if let Some(time) = self.submitted_at.as_deref().and_then(campus_time::parse_time) {
            format!("已提交（{}）", fmt_time(time, now))
        } else if self.is_submitted() {
            "已提交".into()
        } else if self.missing {
            "缺交".into()
        } else if due_passed {
            "未提交（已截止）".into()
        } else {
            "未提交".into()
        };
        if let Some(grade) = self.grade.as_deref().filter(|grade| {
            self.workflow_state.as_deref() == Some("graded") && grade.parse::<f64>().is_err()
        }) {
            text.push_str(&format!("，等级 {grade}"));
        }
        if self.late {
            text.push_str("，迟交");
        }
        if self.attempt.is_some_and(|attempt| attempt > 1) {
            text.push_str(&format!("，第 {} 次提交", self.attempt.unwrap_or_default()));
        }
        text
    }
}

/// 10.0 → "10"，9.5 → "9.5"。
pub(super) fn trim_number(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1e15 {
        format!("{}", value as i64)
    } else {
        format!("{:.2}", value).trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn pagination_only_follows_canvas_itself() {
        let header = r#"<https://oc.sjtu.edu.cn/api/v1/x?page=2>; rel="next",<https://oc.sjtu.edu.cn/api/v1/x?page=1>; rel="first""#;
        let next = next_link(header).unwrap();
        assert_eq!(next, "https://oc.sjtu.edu.cn/api/v1/x?page=2");
        assert!(same_origin(&next, CANVAS_BASE));
        assert!(!same_origin("https://evil.test/api/v1/x?page=2", CANVAS_BASE));
        assert!(!same_origin("http://oc.sjtu.edu.cn/api/v1/x", CANVAS_BASE));
        assert_eq!(next_link(r#"<https://oc.sjtu.edu.cn/x>; rel="last""#), None);
    }

    #[test]
    fn submission_status_reads_like_a_student_would_say_it() {
        let now = campus_time::beijing().with_ymd_and_hms(2026, 11, 25, 12, 0, 0).unwrap();
        let graded = Submission {
            workflow_state: Some("graded".into()),
            submitted_at: Some("2026-11-20T13:03:00Z".into()),
            score: Some(9.5),
            grade: Some("9.5".into()),
            ..Default::default()
        };
        assert_eq!(graded.status(Some(10.0), true, now), "已批改 9.5/10");
        let late = Submission {
            workflow_state: Some("submitted".into()),
            submitted_at: Some("2026-11-20T13:03:00Z".into()),
            late: true,
            attempt: Some(2),
            ..Default::default()
        };
        assert_eq!(late.status(Some(10.0), true, now), "已提交（11月20日 周五 21:03），迟交，第 2 次提交");
        let letter = Submission {
            workflow_state: Some("graded".into()),
            score: Some(93.0),
            grade: Some("A".into()),
            ..Default::default()
        };
        assert_eq!(letter.status(Some(100.0), true, now), "已批改 93/100，等级 A");
        let empty = Submission { workflow_state: Some("unsubmitted".into()), ..Default::default() };
        assert_eq!(empty.status(None, false, now), "未提交");
        assert_eq!(empty.status(None, true, now), "未提交（已截止）");
        assert!(!empty.is_done());
        let missing = Submission { missing: true, ..Default::default() };
        assert_eq!(missing.status(None, true, now), "缺交");
        assert_eq!(trim_number(10.0), "10");
        assert_eq!(trim_number(9.25), "9.25");
        assert_eq!(submission_type_cn("online_upload"), "上传文件");
    }
}
