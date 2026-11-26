use mail_parser::MimeHeaders;

use super::clip;

const MAIL_TIMEOUT_SECS: u64 = 30;
const MAIL_MAX_LIMIT: usize = 50;
const MAIL_DEFAULT_SCAN: usize = 200;
const MAIL_MAX_SCAN: usize = 2000;

struct MailConfig {
    host: String,
    port: u16,
    user: String,
    pass: String,
}

impl MailConfig {
    fn from_env() -> Result<Self, String> {
        let user = std::env::var("EMAIL_USER_ACCOUNT")
            .map_err(|_| "缺少 EMAIL_USER_ACCOUNT".to_string())?;
        let pass = std::env::var("IMAP_PASSWORD")
            .ok()
            .filter(|value| !value.is_empty())
            .or_else(|| std::env::var("EMAIL_USER_PASSWORD").ok())
            .ok_or_else(|| "缺少 IMAP_PASSWORD 或 EMAIL_USER_PASSWORD".to_string())?;
        let host =
            std::env::var("IMAP_HOST").map_err(|_| "缺少 IMAP_HOST（请在 .env 里配置）".to_string())?;
        let port = std::env::var("IMAP_PORT")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(993);
        Ok(Self {
            host,
            port,
            user,
            pass,
        })
    }
}

type MailSession = imap::Session<native_tls::TlsStream<std::net::TcpStream>>;

fn mail_connect(config: &MailConfig) -> Result<MailSession, String> {
    use std::net::ToSocketAddrs;

    let timeout = std::time::Duration::from_secs(MAIL_TIMEOUT_SECS);
    let address = (config.host.as_str(), config.port)
        .to_socket_addrs()
        .map_err(|err| format!("解析 {} 失败: {err}", config.host))?
        .next()
        .ok_or_else(|| format!("{}:{} 没有可用地址", config.host, config.port))?;

    let stream = std::net::TcpStream::connect_timeout(&address, timeout)
        .map_err(|err| format!("连接 {address} 失败: {err}"))?;
    // 不设超时的话，服务器卡住会把整个 ReAct 轮次拖死
    stream
        .set_read_timeout(Some(timeout))
        .map_err(|err| err.to_string())?;
    stream
        .set_write_timeout(Some(timeout))
        .map_err(|err| err.to_string())?;

    let tls = native_tls::TlsConnector::builder()
        .build()
        .map_err(|err| format!("初始化 TLS 失败: {err}"))?;
    let tls_stream = tls
        .connect(&config.host, stream)
        .map_err(|err| format!("TLS 握手失败: {err}"))?;

    imap::Client::new(tls_stream)
        .login(&config.user, &config.pass)
        .map_err(|(err, _)| format!("登录失败（确认用的是客户端授权码而不是登录密码）: {err}"))
}

fn format_addr(addr: &mail_parser::Addr<'_>) -> String {
    match (addr.name.as_deref(), addr.address.as_deref()) {
        // 裸地址时 mail-parser 会把地址同时填进 name，避免打成 "a@b <a@b>"
        (Some(name), Some(mail)) if name != mail => format!("{name} <{mail}>"),
        (_, Some(mail)) => mail.to_string(),
        (Some(name), None) => name.to_string(),
        (None, None) => "(未知发件人)".to_string(),
    }
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// IMAP 的 UID 集合是字符串形式，形如 "12,34,56"。
fn uid_set(uids: &[u32]) -> String {
    uids.iter()
        .map(|uid| uid.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

fn mail_list(
    session: &mut MailSession,
    folder: &str,
    limit: usize,
    unread_only: bool,
) -> Result<String, String> {
    // EXAMINE 而不是 SELECT：只读打开，不会改动任何标记
    session.examine(folder).map_err(|err| format!("打开 {folder} 失败: {err}"))?;

    let query = if unread_only { "UNSEEN" } else { "ALL" };
    let mut uids: Vec<u32> = session
        .uid_search(query)
        .map_err(|err| format!("搜索邮件失败: {err}"))?
        .into_iter()
        .collect();
    uids.sort_unstable();

    let matched = uids.len();
    let targets: Vec<u32> = uids.into_iter().rev().take(limit).collect();
    if targets.is_empty() {
        return Ok(format!("{folder} 里没有符合条件的邮件。"));
    }

    // BODY.PEEK 而不是 BODY：取 header 不会把邮件标成已读
    let uid_set = uid_set(&targets);
    let messages = session
        .uid_fetch(&uid_set, "(FLAGS RFC822.SIZE BODY.PEEK[HEADER])")
        .map_err(|err| format!("读取邮件列表失败: {err}"))?;

    let parser = mail_parser::MessageParser::default();
    let mut text = format!(
        "{folder}：共匹配 {matched} 封，下面是最新的 {} 封\n",
        targets.len()
    );

    for message in messages.iter() {
        let uid = message.uid.unwrap_or(0);
        let seen = message
            .flags()
            .iter()
            .any(|flag| matches!(flag, imap::types::Flag::Seen));
        let size = message.size.unwrap_or(0);
        let parsed = message.header().and_then(|raw| parser.parse(raw));

        let (from, subject, date) = match &parsed {
            Some(parsed) => (
                parsed
                    .from()
                    .and_then(|list| list.first())
                    .map(format_addr)
                    .unwrap_or_default(),
                parsed.subject().unwrap_or("(无主题)").to_string(),
                parsed
                    .date()
                    .map(|date| date.to_rfc3339())
                    .unwrap_or_else(|| "(无日期)".to_string()),
            ),
            None => ("(解析失败)".to_string(), "(解析失败)".to_string(), String::new()),
        };

        text.push_str(&format!(
            "  UID {uid} [{}] {} 字节\n    发件人：{}\n    主题：{}\n    时间：{}\n",
            if seen { "已读" } else { "未读" },
            size,
            one_line(&from),
            one_line(&subject),
            date
        ));
    }

    text.push_str("\n要读某一封的正文，用 action=read 和对应的 uid。");
    Ok(clip(&text))
}

fn mail_read(session: &mut MailSession, folder: &str, uid: u32) -> Result<String, String> {
    session.examine(folder).map_err(|err| format!("打开 {folder} 失败: {err}"))?;

    let messages = session
        .uid_fetch(uid.to_string(), "(FLAGS RFC822.SIZE BODY.PEEK[])")
        .map_err(|err| format!("读取邮件失败: {err}"))?;

    let raw = messages
        .iter()
        .find_map(|message| message.body())
        .ok_or_else(|| format!("{folder} 里没有 uid 为 {uid} 的邮件"))?;

    let parsed = mail_parser::MessageParser::default()
        .parse(raw)
        .ok_or_else(|| "这封邮件解析失败".to_string())?;

    let mut text = String::new();
    text.push_str(&format!(
        "主题：{}\n",
        parsed.subject().unwrap_or("(无主题)")
    ));
    text.push_str(&format!(
        "发件人：{}\n",
        parsed
            .from()
            .and_then(|list| list.first())
            .map(format_addr)
            .unwrap_or_else(|| "(未知)".to_string())
    ));
    if let Some(date) = parsed.date() {
        text.push_str(&format!("时间：{}\n", date.to_rfc3339()));
    }
    let to: Vec<String> = parsed
        .to()
        .map(|list| list.iter().map(format_addr).collect())
        .unwrap_or_default();
    if !to.is_empty() {
        text.push_str(&format!("收件人：{}\n", one_line(&to.join(", "))));
    }

    let attachments: Vec<_> = parsed.attachments().collect();
    if !attachments.is_empty() {
        text.push_str("附件（可用 action=attachment 加序号下载）：\n");
        for (index, attachment) in attachments.iter().enumerate() {
            text.push_str(&format!(
                "  {}. {}（{} 字节）\n",
                index + 1,
                attachment.attachment_name().unwrap_or("(未命名)"),
                attachment.len()
            ));
        }
    }

    // body_text 在只有 HTML 的邮件里会自动转成纯文本，不用自己剥标签
    let body = parsed
        .body_text(0)
        .map(|body| body.trim().to_string())
        .filter(|body| !body.is_empty())
        .unwrap_or_else(|| "(没有可显示的正文)".to_string());
    text.push_str("\n———— 正文 ————\n");
    text.push_str(&body);

    Ok(clip(&text))
}

fn mail_folders(session: &mut MailSession) -> Result<String, String> {
    let folders = session
        .list(None, Some("*"))
        .map_err(|err| format!("列出文件夹失败: {err}"))?;

    let mut text = String::from("邮箱文件夹：\n");
    for folder in folders.iter() {
        text.push_str(&format!("  {}\n", folder.name()));
    }
    Ok(text)
}

/// 关键词同时匹配发件人和主题，不区分大小写。
fn header_matches(keyword: &str, from: &str, subject: &str) -> bool {
    let keyword = keyword.to_lowercase();
    from.to_lowercase().contains(&keyword) || subject.to_lowercase().contains(&keyword)
}

/// IMAP 的 SEARCH 处理中文需要服务器支持 CHARSET，各家实现不一；这里取最近 scan 封的
/// 邮件头在本地匹配，结果稳定。
fn mail_search(session: &mut MailSession, folder: &str, keyword: &str, limit: usize, scan: usize) -> Result<String, String> {
    session.examine(folder).map_err(|err| format!("打开 {folder} 失败: {err}"))?;
    let mut uids: Vec<u32> = session
        .uid_search("ALL")
        .map_err(|err| format!("搜索邮件失败: {err}"))?
        .into_iter()
        .collect();
    uids.sort_unstable();
    let scanned: Vec<u32> = uids.iter().rev().take(scan).copied().collect();
    if scanned.is_empty() {
        return Ok(format!("{folder} 里没有邮件。"));
    }
    let messages = session
        .uid_fetch(uid_set(&scanned), "(FLAGS BODY.PEEK[HEADER])")
        .map_err(|err| format!("读取邮件头失败: {err}"))?;
    let parser = mail_parser::MessageParser::default();
    let mut hits: Vec<(u32, String)> = Vec::new();
    for message in messages.iter() {
        let Some(parsed) = message.header().and_then(|raw| parser.parse(raw)) else { continue };
        let from = parsed.from().and_then(|list| list.first()).map(format_addr).unwrap_or_default();
        let subject = parsed.subject().unwrap_or("(无主题)").to_string();
        if !header_matches(keyword, &from, &subject) {
            continue;
        }
        let uid = message.uid.unwrap_or(0);
        let seen = message.flags().iter().any(|flag| matches!(flag, imap::types::Flag::Seen));
        let date = parsed.date().map(|date| date.to_rfc3339()).unwrap_or_default();
        hits.push((
            uid,
            format!(
                "  UID {uid} [{}]\n    发件人：{}\n    主题：{}\n    时间：{date}\n",
                if seen { "已读" } else { "未读" },
                one_line(&from),
                one_line(&subject)
            ),
        ));
    }
    hits.sort_by_key(|(uid, _)| std::cmp::Reverse(*uid));
    let total = hits.len();
    let mut text = format!(
        "在 {folder} 最近 {} 封邮件中，发件人或主题包含「{keyword}」的有 {total} 封",
        scanned.len()
    );
    if total > limit {
        text.push_str(&format!("，下面是最新的 {limit} 封"));
    }
    text.push_str("：\n");
    for (_, line) in hits.iter().take(limit) {
        text.push_str(line);
    }
    if total == 0 && scanned.len() < uids.len() {
        text.push_str(&format!("没有找到。可以增大 scan（当前 {scan}）搜索更早的邮件。\n"));
    }
    text.push_str("\n要读某一封的正文，用 action=read 和对应的 uid。");
    Ok(clip(&text))
}

/// 按序号（从 1 开始）或文件名片段选附件。
fn pick_attachment(names: &[String], which: &str) -> Result<usize, String> {
    let which = which.trim();
    if names.is_empty() {
        return Err("这封邮件没有附件".into());
    }
    if let Ok(number) = which.parse::<usize>() {
        return (1..=names.len())
            .contains(&number)
            .then(|| number - 1)
            .ok_or_else(|| format!("附件序号应在 1 到 {} 之间", names.len()));
    }
    let needle = which.to_lowercase();
    let matched: Vec<usize> = (0..names.len())
        .filter(|index| names[*index].to_lowercase().contains(&needle))
        .collect();
    match matched.as_slice() {
        [index] => Ok(*index),
        [] => Err(format!("没有名称包含「{which}」的附件，附件有：{}", names.join("、"))),
        _ => Err(format!("有多个附件匹配「{which}」，请改用序号：{}", names.join("、"))),
    }
}

/// 附件名来自邮件，只取最后一段文件名，防止“../”之类的路径跳出下载目录。
fn attachment_target(name: &str, uid: u32, index: usize, save_path: Option<&str>) -> std::path::PathBuf {
    if let Some(path) = save_path.map(str::trim).filter(|path| !path.is_empty()) {
        return std::path::PathBuf::from(path);
    }
    let file = super::downloads::file_name(name.trim(), format!("mail-{uid}-attachment-{}", index + 1));
    super::downloads::directory().join(file)
}

fn mail_attachment(session: &mut MailSession, folder: &str, uid: u32, which: &str, save_path: Option<&str>) -> Result<String, String> {
    session.examine(folder).map_err(|err| format!("打开 {folder} 失败: {err}"))?;
    let messages = session
        .uid_fetch(uid.to_string(), "BODY.PEEK[]")
        .map_err(|err| format!("读取邮件失败: {err}"))?;
    let raw = messages
        .iter()
        .find_map(|message| message.body())
        .ok_or_else(|| format!("{folder} 里没有 uid 为 {uid} 的邮件"))?;
    let parsed = mail_parser::MessageParser::default()
        .parse(raw)
        .ok_or_else(|| "这封邮件解析失败".to_string())?;
    let attachments: Vec<_> = parsed.attachments().collect();
    let names: Vec<String> = attachments
        .iter()
        .map(|attachment| attachment.attachment_name().unwrap_or("(未命名)").to_string())
        .collect();
    let index = pick_attachment(&names, which)?;
    let target = attachment_target(&names[index], uid, index, save_path);
    if let Some(parent) = target.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|err| format!("创建目录 {} 失败: {err}", parent.display()))?;
    }
    let contents = attachments[index].contents();
    std::fs::write(&target, contents).map_err(|err| format!("写入 {} 失败: {err}", target.display()))?;
    Ok(format!(
        "已把附件「{}」保存到 {}（{} 字节）。PDF 可用 pdf_analysis 阅读，文本文件可用 read。",
        names[index],
        target.display(),
        contents.len()
    ))
}

struct MailRequest {
    action: String,
    folder: String,
    limit: usize,
    uid: Option<u32>,
    unread_only: bool,
    keyword: Option<String>,
    scan: usize,
    attachment: Option<String>,
    save_path: Option<String>,
}

fn run_mail(request: MailRequest) -> Result<String, String> {
    // 先校验参数，避免无意义地连接邮箱
    let needs_uid = |action: &str| {
        request
            .uid
            .ok_or_else(|| format!("action={action} 时必须给出 uid，可先用 action=list 或 search 查看"))
    };
    match request.action.as_str() {
        "read" | "attachment" => {
            needs_uid(&request.action)?;
        }
        "search" if request.keyword.is_none() => return Err("action=search 时需要 keyword".into()),
        "list" | "folders" | "search" => {}
        other => return Err(format!("未知的 action「{other}」，只支持 list / read / search / attachment / folders")),
    }
    let config = MailConfig::from_env()?;
    let mut session = mail_connect(&config)?;
    let folder = request.folder.as_str();
    let result = match request.action.as_str() {
        "list" => mail_list(&mut session, folder, request.limit, request.unread_only),
        "read" => mail_read(&mut session, folder, needs_uid("read")?),
        "search" => mail_search(&mut session, folder, request.keyword.as_deref().unwrap_or_default(), request.limit, request.scan),
        "attachment" => mail_attachment(
            &mut session,
            folder,
            needs_uid("attachment")?,
            request.attachment.as_deref().unwrap_or("1"),
            request.save_path.as_deref(),
        ),
        _ => mail_folders(&mut session),
    };
    let _ = session.logout();
    result
}

/// 今日简报用：收件箱未读总数，以及最新几封未读的“发件人 · 主题”。只读，不改变已读状态。
pub(super) async fn unread_brief(limit: usize) -> Result<(usize, Vec<String>), String> {
    tokio::task::spawn_blocking(move || {
        let config = MailConfig::from_env()?;
        let mut session = mail_connect(&config)?;
        let mut run = || -> Result<(usize, Vec<String>), String> {
            session.examine("INBOX").map_err(|err| format!("打开 INBOX 失败: {err}"))?;
            let mut uids: Vec<u32> = session
                .uid_search("UNSEEN")
                .map_err(|err| format!("搜索未读邮件失败: {err}"))?
                .into_iter()
                .collect();
            uids.sort_unstable();
            let total = uids.len();
            let latest: Vec<u32> = uids.into_iter().rev().take(limit).collect();
            if latest.is_empty() {
                return Ok((total, Vec::new()));
            }
            let messages = session
                .uid_fetch(uid_set(&latest), "(BODY.PEEK[HEADER])")
                .map_err(|err| format!("读取邮件列表失败: {err}"))?;
            let parser = mail_parser::MessageParser::default();
            let mut lines: Vec<(u32, String)> = messages
                .iter()
                .map(|message| {
                    let parsed = message.header().and_then(|raw| parser.parse(raw));
                    let from = parsed
                        .as_ref()
                        .and_then(|parsed| parsed.from().and_then(|list| list.first()).map(format_addr))
                        .unwrap_or_else(|| "(未知发件人)".into());
                    let subject = parsed
                        .as_ref()
                        .and_then(|parsed| parsed.subject().map(str::to_string))
                        .unwrap_or_else(|| "(无主题)".into());
                    (message.uid.unwrap_or(0), format!("{} · {}", one_line(&from), one_line(&subject)))
                })
                .collect();
            lines.sort_by_key(|(uid, _)| std::cmp::Reverse(*uid));
            Ok((total, lines.into_iter().map(|(_, line)| line).collect()))
        };
        let result = run();
        let _ = session.logout();
        result
    })
    .await
    .map_err(|err| format!("邮件任务失败: {err}"))?
}

pub(super) async fn mail_fetch(arguments: serde_json::Value) -> Result<String, String> {
    let action = arguments["action"]
        .as_str()
        .unwrap_or("list")
        .trim()
        .to_lowercase();
    let folder = arguments["folder"]
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("INBOX")
        .to_string();
    let limit = match arguments["limit"].as_u64() {
        Some(value) => (value as usize).clamp(1, MAIL_MAX_LIMIT),
        None => 10,
    };
    let uid = match &arguments["uid"] {
        serde_json::Value::Number(number) => number.as_u64().map(|value| value as u32),
        serde_json::Value::String(text) => text.trim().parse().ok(),
        _ => None,
    };
    let text_arg = |key: &str| {
        arguments[key]
            .as_str()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    };
    let request = MailRequest {
        action,
        folder,
        limit,
        uid,
        unread_only: arguments["unread_only"].as_bool().unwrap_or(false),
        keyword: text_arg("keyword"),
        scan: arguments["scan"].as_u64().map_or(MAIL_DEFAULT_SCAN, |scan| (scan as usize).clamp(1, MAIL_MAX_SCAN)),
        attachment: text_arg("attachment").or_else(|| arguments["attachment"].as_u64().map(|n| n.to_string())),
        save_path: text_arg("save_path"),
    };

    tokio::task::spawn_blocking(move || run_mail(request))
        .await
        .map_err(|err| format!("邮件任务失败: {err}"))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_matches_sender_or_subject_ignoring_case() {
        assert!(header_matches("canvas", "Canvas@SJTU <notifications@oc.sjtu.edu.cn>", "作业提醒"));
        assert!(header_matches("奖学金", "学生处 <xsc@sjtu.edu.cn>", "关于2026年奖学金评定的通知"));
        assert!(!header_matches("讲座", "教务处", "选课通知"));
    }

    #[test]
    fn attachments_are_picked_by_number_or_name_and_saved_safely() {
        let names = vec!["实验指导书.pdf".to_string(), "评分标准.xlsx".to_string(), "实验数据.pdf".to_string()];
        assert_eq!(pick_attachment(&names, "2"), Ok(1));
        assert_eq!(pick_attachment(&names, "评分"), Ok(1));
        assert!(pick_attachment(&names, "实验").unwrap_err().contains("多个附件"));
        assert!(pick_attachment(&names, "4").unwrap_err().contains("1 到 3"));
        assert!(pick_attachment(&[], "1").unwrap_err().contains("没有附件"));
        let target = attachment_target("../../.ssh/authorized_keys", 7, 0, None);
        let downloads = super::super::downloads::directory();
        assert_eq!(target, downloads.join("authorized_keys"));
        assert_eq!(attachment_target("..", 7, 1, None), downloads.join("mail-7-attachment-2"));
        assert_eq!(attachment_target("a.pdf", 7, 0, Some("/tmp/x.pdf")), std::path::PathBuf::from("/tmp/x.pdf"));
    }

    #[tokio::test]
    async fn bad_mail_requests_fail_before_connecting() {
        let error = mail_fetch(serde_json::json!({"action": "search"})).await.unwrap_err();
        assert!(error.contains("keyword"), "{error}");
        let error = mail_fetch(serde_json::json!({"action": "attachment"})).await.unwrap_err();
        assert!(error.contains("uid"), "{error}");
        let error = mail_fetch(serde_json::json!({"action": "delete"})).await.unwrap_err();
        assert!(error.contains("未知的 action"), "{error}");
    }
}
