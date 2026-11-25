//! Outgoing messages are staged by tools and delivered only by a user reply in the same chat.
use std::collections::HashMap;
use std::hash::{BuildHasher, RandomState};
use std::process::Stdio;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use super::{USER_NOTICE_MARK, shuiyuan};
use crate::sso::{FetchRequest, campus_fetch};

tokio::task_local! { pub(crate) static SESSION: String; }

enum Draft {
    Mail {
        to: String,
        subject: String,
        body: String,
    },
    Topic {
        category: u64,
        title: String,
        body: String,
    },
    Reply {
        topic: u64,
        body: String,
    },
}

struct Pending {
    code: String,
    created: Instant,
    draft: Draft,
}

fn drafts() -> &'static Mutex<HashMap<String, Pending>> {
    static DRAFTS: OnceLock<Mutex<HashMap<String, Pending>>> = OnceLock::new();
    DRAFTS.get_or_init(Default::default)
}

const LIFETIME: Duration = Duration::from_secs(15 * 60);

fn field(args: &Value, key: &str, header: bool) -> Result<String, String> {
    let raw = args[key].as_str().ok_or_else(|| format!("缺少 {key}"))?;
    let text = raw.replace("\r\n", "\n");
    let max_bytes = if header { 800 } else { 32_000 };
    if text.trim().is_empty() || text.len() > max_bytes {
        return Err(format!("{key} 不能为空，且最多支持 {max_bytes} 字节"));
    }
    if text
        .chars()
        .any(|ch| ch.is_control() && (header || !matches!(ch, '\n' | '\t')))
    {
        return Err(format!("{key} 含有无效的控制字符"));
    }
    Ok(if header { text.trim().to_owned() } else { text })
}

fn destination(args: &Value, key: &str) -> Result<u64, String> {
    args[key]
        .as_u64()
        .filter(|id| *id > 0)
        .ok_or_else(|| format!("请指定有效的 {key}"))
}

fn stage(draft: Draft) -> Result<String, String> {
    let description = match &draft {
        Draft::Mail { to, subject, body } => format!("收件人：{to}\n主题：{subject}\n\n{body}"),
        Draft::Topic {
            category,
            title,
            body,
        } => format!(
            "水源新主题 · 分区 #{}（{}/c/{}）\n标题：{}\n\n{}",
            category,
            shuiyuan::SHUIYUAN,
            category,
            title,
            body
        ),
        Draft::Reply { topic, body } => format!(
            "水源回帖 · 主题 {}/t/{}\n\n{}",
            shuiyuan::SHUIYUAN,
            topic,
            body
        ),
    };
    let Ok(session) = SESSION.try_with(Clone::clone) else {
        return Ok(format!(
            "【预览】\n{description}\n\n要发送这份内容，请在聊天会话中操作。"
        ));
    };
    let mut pending = drafts().lock().map_err(|_| "待发草稿不可用")?;
    pending.retain(|_, entry| entry.created.elapsed() < LIFETIME);
    if pending.len() >= 128 && !pending.contains_key(&session) {
        return Err("待发草稿已达到上限，请稍后再试".into());
    }
    let code = format!("{:08X}", RandomState::new().hash_one(&session) as u32);
    pending.insert(
        session,
        Pending {
            code: code.clone(),
            created: Instant::now(),
            draft,
        },
    );
    Ok(format!(
        "待发预览已直接展示给用户，等待用户输入确认口令。本轮结束。{USER_NOTICE_MARK}【待发预览】\n{description}\n\n核对后回复“确认发送 {code}”。此草稿保留 15 分钟。"
    ))
}

pub(super) async fn send_email(args: Value) -> Result<String, String> {
    let to = field(&args, "to", true)?;
    static ADDRESS: OnceLock<regex::Regex> = OnceLock::new();
    let address = ADDRESS.get_or_init(|| regex::Regex::new(
        r"^[A-Za-z0-9!#$%&'*+/=?^_`{|}~-]+(?:\.[A-Za-z0-9!#$%&'*+/=?^_`{|}~-]+)*@(?:[A-Za-z0-9](?:[A-Za-z0-9-]{0,61}[A-Za-z0-9])?\.)+[A-Za-z0-9](?:[A-Za-z0-9-]{0,61}[A-Za-z0-9])?$"
    ).expect("valid mailbox regex"));
    if to.len() > 254
        || !address.is_match(&to)
        || to.split('@').next().is_none_or(|local| local.len() > 64)
    {
        return Err("请提供一个完整的收件邮箱地址，如 teacher@sjtu.edu.cn".into());
    }
    stage(Draft::Mail {
        to,
        subject: field(&args, "subject", true)?,
        body: field(&args, "body", false)?,
    })
}

pub(super) async fn shuiyuan_post(args: Value) -> Result<String, String> {
    match args["action"].as_str() {
        Some("categories") => {
            let data = shuiyuan::fetch_json("/categories.json?include_subcategories=true").await?;
            let categories = data["category_list"]["categories"]
                .as_array()
                .ok_or("水源未返回分区列表")?;
            let mut lines = Vec::new();
            category_lines(categories, "", &mut lines);
            Ok(format!("水源分区（名称和 ID）\n{}", lines.join("\n")))
        }
        Some("topic") if args["topic_id"].is_null() => stage(Draft::Topic {
            category: destination(&args, "category_id")?,
            title: field(&args, "title", true)?,
            body: field(&args, "body", false)?,
        }),
        Some("reply") if args["category_id"].is_null() && args["title"].is_null() => {
            stage(Draft::Reply {
                topic: destination(&args, "topic_id")?,
                body: field(&args, "body", false)?,
            })
        }
        _ => Err(
            "action 须为 categories、topic（指定 category_id 和 title）或 reply（指定 topic_id）"
                .into(),
        ),
    }
}

fn category_lines(categories: &[Value], parent: &str, lines: &mut Vec<String>) {
    for cat in categories {
        let (Some(id), Some(name)) = (cat["id"].as_u64(), cat["name"].as_str()) else {
            continue;
        };
        let path = if parent.is_empty() {
            name.to_owned()
        } else {
            format!("{parent} / {name}")
        };
        lines.push(format!("{path} · #{id}"));
        if let Some(children) = cat["subcategory_list"].as_array() {
            category_lines(children, &path, lines);
        }
    }
}

/// Only the literal current user input reaches this entry point. Neither tool arguments nor
/// historical messages can confirm a draft. Taking the entry before I/O also makes it single-use.
pub(crate) async fn consume_confirmation(prompt: &str) -> Option<Result<String, String>> {
    let code = prompt.trim().strip_prefix("确认发送 ")?.trim();
    if code.len() != 8 || !code.bytes().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let session = SESSION.try_with(Clone::clone).ok()?;
    let draft = {
        let Ok(mut pending) = drafts().lock() else {
            return Some(Err("待发草稿不可用".into()));
        };
        pending.retain(|_, entry| entry.created.elapsed() < LIFETIME);
        if !pending
            .get(&session)
            .is_some_and(|entry| entry.code.eq_ignore_ascii_case(code))
        {
            return Some(Err(
                "此口令与当前会话的待发草稿不匹配，或已经使用、过期。".into()
            ));
        }
        pending.remove(&session).expect("matched under lock").draft
    };
    Some(match draft {
        Draft::Mail { to, subject, body } => deliver_mail(&to, &subject, &body).await,
        Draft::Topic {
            category,
            title,
            body,
        } => {
            publish(&[
                ("category", category.to_string()),
                ("title", title),
                ("raw", body),
            ])
            .await
        }
        Draft::Reply { topic, body } => {
            publish(&[("topic_id", topic.to_string()), ("raw", body)]).await
        }
    })
}

async fn deliver_mail(to: &str, subject: &str, body: &str) -> Result<String, String> {
    let python = std::env::var_os("AGENT_SSO_PYTHON").unwrap_or_else(|| "python3".into());
    let mut child = Command::new(python)
        .args(["-I", "-c", include_str!("smtp_send.py")])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| "无法启动本地 Python 3 邮件助手")?;
    let input = json!({"to": to, "subject": subject, "body": body}).to_string();
    let mut stdin = child.stdin.take().ok_or("无法打开邮件助手输入")?;
    stdin
        .write_all(input.as_bytes())
        .await
        .map_err(|_| "无法将草稿交给邮件助手")?;
    drop(stdin);
    let uncertain = "邮件发送结果未知，请先向收件人或邮箱管理员核实，再决定是否重发。";
    let output = tokio::time::timeout(Duration::from_secs(90), child.wait_with_output())
        .await
        .map_err(|_| uncertain)?
        .map_err(|_| uncertain)?;
    let result: Value = serde_json::from_slice(&output.stdout).map_err(|_| uncertain)?;
    if result["accepted"] == true && output.status.success() {
        Ok(format!("邮件服务器已接收发往 {to} 的邮件《{subject}》。"))
    } else {
        Err(result["error"].as_str().unwrap_or(uncertain).to_string())
    }
}

async fn publish(form: &[(&str, String)]) -> Result<String, String> {
    let entry = format!("{}/", shuiyuan::SHUIYUAN);
    let request = FetchRequest::post("/posts.json", form);
    let response = campus_fetch(&entry, &[request], 1_500_000)
        .await
        .map_err(|err| format!("未取得水源回执，请先到水源核实发帖状态。{err}"))?
        .pop()
        .ok_or("水源未返回回执，请先核实帖子状态")?;
    let uncertain = "水源发帖结果未知，请先在自己的水源动态中核实，再决定是否重发。";
    if response.status == 0 || response.status >= 500 {
        return Err(uncertain.into());
    }
    if let Some(error) = response.error {
        return Err(error);
    }
    let data = serde_json::from_str::<Value>(&response.text).unwrap_or_default();
    if !(200..300).contains(&response.status)
        || data["success"] == false
        || data["errors"].as_array().is_some_and(|errors| !errors.is_empty())
    {
        let reason = data["errors"]
            .as_array()
            .map(|errors| {
                errors
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join("；")
            })
            .unwrap_or_default();
        return Err(format!(
            "水源拒绝发帖（HTTP {}） {}",
            response.status,
            reason.chars().take(500).collect::<String>()
        ));
    }
    if data["action"] == "enqueued" {
        return Ok("水源已收到帖子，正在等待版主审核。".into());
    }
    let post = data.get("post").unwrap_or(&data);
    let topic = post["topic_id"].as_u64().ok_or(uncertain)?;
    let number = post["post_number"].as_u64().ok_or(uncertain)?;
    Ok(format!("已发布：{}/t/{topic}/{number}", shuiyuan::SHUIYUAN))
}
