use mail_parser::MimeHeaders;

use super::clip;

const MAIL_TIMEOUT_SECS: u64 = 30;
const MAIL_MAX_LIMIT: usize = 50;

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
        text.push_str("附件：\n");
        for attachment in attachments {
            text.push_str(&format!(
                "  - {}（{} 字节）\n",
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

fn run_mail(
    action: String,
    folder: String,
    limit: usize,
    uid: Option<u32>,
    unread_only: bool,
) -> Result<String, String> {
    let config = MailConfig::from_env()?;
    let mut session = mail_connect(&config)?;

    let result = match action.as_str() {
        "list" => mail_list(&mut session, &folder, limit, unread_only),
        "read" => {
            let uid = uid.ok_or_else(|| "action=read 时必须给出 uid，可先用 action=list 查看".to_string())?;
            mail_read(&mut session, &folder, uid)
        }
        "folders" => mail_folders(&mut session),
        other => Err(format!("未知的 action「{other}」，只支持 list / read / folders")),
    };

    let _ = session.logout();
    result
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
    let unread_only = arguments["unread_only"].as_bool().unwrap_or(false);

    tokio::task::spawn_blocking(move || run_mail(action, folder, limit, uid, unread_only))
        .await
        .map_err(|err| format!("邮件任务失败: {err}"))?
}
