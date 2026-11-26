//! Canvas course navigation → the SJTU LTI student view → its read-only attendance list.
//! The student table labels createdDate "发起签到时间" and translates the learner's state.
use chrono::{DateTime, NaiveDateTime, TimeZone, Utc};
use reqwest::{Client, StatusCode};
use serde::Deserialize;
use serde_json::{Value, json};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use super::campus_time::beijing;
use super::canvas::Canvas;

const RECORDS: &str = "https://mlearning.sjtu.edu.cn/lms-lti-rollcall-sjtu/sign/records/";
const LOGIN_ERROR: &str = "未能打开此课程的点名签到学生页，请在 Canvas 中确认自己有权查看。";

pub(super) struct AttendanceRecord {
    pub(super) id: String,
    pub(super) started_at: DateTime<Utc>,
    pub(super) status: Option<String>,
}

pub(super) struct AttendanceSource {
    course_id: u64,
    session: Option<StudentSession>,
    successful_logins: u8,
}

struct StudentSession {
    client: Client,
    ticket: Ticket,
}

#[derive(Deserialize)]
struct Ticket {
    course_code: String,
    token: String,
}

#[derive(Deserialize)]
struct Tab {
    label: String,
    id: String,
    #[serde(default)]
    hidden: bool,
}

impl AttendanceSource {
    pub(super) fn new(course_id: u64) -> Self {
        Self {
            course_id,
            session: None,
            successful_logins: 0,
        }
    }

    pub(super) async fn records(&mut self) -> Result<Vec<AttendanceRecord>, String> {
        loop {
            if self.session.is_none() {
                if self.successful_logins >= 2 {
                    return Err("点名签到登录已过期，请检查 Canvas 登录状态。".into());
                }
                self.session = Some(StudentSession::open(self.course_id).await?);
                self.successful_logins += 1;
            }
            match self.session.as_ref().unwrap().read().await {
                Ok(records) => return Ok(records),
                Err(ReadError::Failure(reason)) => return Err(reason.into()),
                Err(ReadError::Expired) => self.session = None,
            }
        }
    }
}

enum ReadError {
    Expired,
    Failure(&'static str),
}

impl StudentSession {
    async fn open(course_id: u64) -> Result<Self, String> {
        let canvas = Canvas::from_env()?;
        let tabs: Vec<Tab> = canvas
            .get_all(
                &format!("/api/v1/courses/{course_id}/tabs"),
                &[],
                "课程导航",
            )
            .await
            .map_err(|_| "无法读取本人 Canvas 课程菜单，请检查课程权限与令牌。")?;
        let tool = tabs
            .iter()
            .find(|tab| !tab.hidden && tab.label.trim() == "点名签到")
            .and_then(|tab| tab.id.strip_prefix("context_external_tool_"))
            .and_then(|id| id.parse::<u64>().ok())
            .ok_or("此 Canvas 课程菜单未开放「点名签到」。")?;
        #[derive(Deserialize)]
        struct Launch {
            url: String,
        }
        let launch: Launch = canvas
            .get(
                &format!("/api/v1/courses/{course_id}/external_tools/sessionless_launch"),
                &[
                    ("id", tool.to_string()),
                    ("launch_type", "course_navigation".into()),
                ],
                "点名签到学生页",
            )
            .await
            .map_err(|_| LOGIN_ERROR)?;
        let python = std::env::var_os("AGENT_SSO_PYTHON").unwrap_or_else(|| "python3".into());
        let mut command = Command::new(python);
        command
            .args(["-I", "-c", include_str!("attendance_source.py")])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        for key in [
            "OPENAI_API_KEY",
            "CANVAS_API_TOKEN",
            "JCOURSE_EMAIL",
            "JCOURSE_PASSWORD",
            "EMAIL_USER_ACCOUNT",
            "EMAIL_USER_PASSWORD",
            "IMAP_PASSWORD",
            "SMTP_PASSWORD",
        ] {
            command.env_remove(key);
        }
        let mut child = command
            .spawn()
            .map_err(|_| "无法启动 Python 3，请检查 AGENT_SSO_PYTHON。")?;
        let input = json!({ "course_id": course_id, "url": launch.url }).to_string();
        let mut stdin = child.stdin.take().ok_or(LOGIN_ERROR)?;
        stdin
            .write_all(input.as_bytes())
            .await
            .map_err(|_| LOGIN_ERROR)?;
        drop(stdin);
        let output = tokio::time::timeout(Duration::from_secs(20), child.wait_with_output())
            .await
            .map_err(|_| LOGIN_ERROR)?
            .map_err(|_| LOGIN_ERROR)?;
        if !output.status.success() {
            return Err(LOGIN_ERROR.into());
        }
        let ticket = serde_json::from_slice(&output.stdout).map_err(|_| LOGIN_ERROR)?;
        let client = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(8))
            .build()
            .map_err(|_| "无法创建签到查询连接。")?;
        Ok(Self { client, ticket })
    }

    async fn read(&self) -> Result<Vec<AttendanceRecord>, ReadError> {
        const SCHEMA: ReadError =
            ReadError::Failure("课堂签到学生页的记录格式发生变化，无法可靠判断。");
        let mut page = 1_u64;
        let mut total = 0;
        let mut records = Vec::new();
        loop {
            let response = self
                .client
                .get(RECORDS)
                .header("Authorization", &self.ticket.token)
                .header("request-ajax", "true")
                .query(&[
                    ("courseCode", self.ticket.course_code.clone()),
                    ("pageSize", "100".into()),
                    ("pageNum", page.to_string()),
                    ("total", total.to_string()),
                ])
                .send()
                .await
                .map_err(|_| ReadError::Failure("连接课堂签到记录超时或失败。"))?;
            if matches!(
                response.status(),
                StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
            ) || response.status().is_redirection()
            {
                return Err(ReadError::Expired);
            }
            if !response.status().is_success() {
                return Err(ReadError::Failure("课堂签到服务暂时不可用。"));
            }
            let content: Value = response.json().await.map_err(|_| SCHEMA)?;
            for key in ["code", "resultCode"] {
                if matches!(content[key].as_str(), Some("401" | "403"))
                    || matches!(content[key].as_u64(), Some(401 | 403))
                {
                    return Err(ReadError::Expired);
                }
            }
            if content["resultCode"] != "200" && content["resultCode"] != 200 {
                return Err(ReadError::Failure("课堂签到服务未返回本人记录。"));
            }
            let body = &content["body"];
            if body["pageNum"].as_u64() != Some(page) {
                return Err(SCHEMA);
            }
            let pages = body["totalPage"].as_u64().ok_or(SCHEMA)?;
            total = body["total"].as_u64().ok_or(SCHEMA)?;
            for row in body["dataList"].as_array().ok_or(SCHEMA)? {
                records.push(parse_row(row).ok_or(SCHEMA)?);
            }
            if page >= pages {
                return Ok(records);
            }
            page += 1;
        }
    }
}

fn issuance_time(value: &Value) -> Option<DateTime<Utc>> {
    let epoch = value
        .as_i64()
        .or_else(|| value.as_str()?.parse::<i64>().ok());
    if let Some(n) = epoch {
        return match n {
            1_000_000_000..=9_999_999_999 => DateTime::from_timestamp(n, 0),
            1_000_000_000_000..=9_999_999_999_999 => DateTime::from_timestamp_millis(n),
            _ => None,
        };
    }
    let value = value.as_str()?.trim();
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|t| t.to_utc())
        .or_else(|| {
            [
                "%Y-%m-%d %H:%M:%S%.f",
                "%Y-%m-%dT%H:%M:%S%.f",
                "%Y-%m-%d %H:%M",
            ]
            .iter()
            .find_map(|fmt| NaiveDateTime::parse_from_str(value, fmt).ok())
            .and_then(|local| beijing().from_local_datetime(&local).single())
            .map(|time| time.to_utc())
        })
}

fn parse_row(row: &Value) -> Option<AttendanceRecord> {
    let started_at = issuance_time(&row["createdDate"])?;
    let id = match &row["id"] {
        Value::String(id) if !id.is_empty() => id.clone(),
        Value::Number(id) => id.to_string(),
        _ => started_at.to_rfc3339(),
    };
    let status = match row["state"].as_str() {
        Some("NORMAL") => Some("已签到"),
        Some("AMEND") => Some("补签"),
        Some("UNNORMAL") => Some("未签到"),
        Some("TRUANT") => Some("旷课"),
        Some("LATE") => Some("迟到"),
        Some("SICK_LEAVE") => Some("病假"),
        Some("PLEAVE") => Some("请假"),
        Some("PERSONAL_LEAVE") => Some("事假"),
        Some("LEAVE_EARLY") => Some("早退"),
        Some("ETC") => Some("其他"),
        _ => None,
    }
    .map(str::to_owned);
    Some(AttendanceRecord {
        id,
        started_at,
        status,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn student_row_uses_the_issuance_time_and_own_state() {
        let row = json!({"createdDate":"2026-11-27 08:00:05","state":"UNNORMAL","id":42});
        let record = parse_row(&row).unwrap();
        assert_eq!(record.id, "42");
        assert_eq!(record.started_at.to_rfc3339(), "2026-11-27T00:00:05+00:00");
        assert_eq!(record.status.as_deref(), Some("未签到"));
        assert_eq!(
            issuance_time(&json!(record.started_at.timestamp())),
            Some(record.started_at)
        );
        assert_eq!(
            issuance_time(&json!(record.started_at.timestamp_millis())),
            Some(record.started_at)
        );
        assert_eq!(issuance_time(&json!(20261127)), None);
        assert!(parse_row(&json!({"state":"NORMAL"})).is_none());
    }
}
