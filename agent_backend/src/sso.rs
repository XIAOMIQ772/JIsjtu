use std::path::PathBuf;
use std::process::Stdio;
use std::sync::OnceLock;
use std::time::Duration;

use anyhow::{Context, bail};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::sync::Mutex;

// Embed the helper so packaged binaries do not depend on the source tree.
const BROWSER_HELPER: &str = include_str!("./sso/browser.py");
const PROCESS_GUARD: &str = include_str!("./sso/process_guard.py");
static BROWSER_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

#[derive(Deserialize)]
struct HelperResult {
    ok: bool,
    #[serde(default)]
    reused: bool,
    #[serde(default)]
    headless: bool,
    #[serde(default)]
    page: Option<Value>,
    #[serde(default)]
    page_closed: bool,
    #[serde(default)]
    error: String,
}

fn credentials() -> anyhow::Result<(String, String)> {
    let account = std::env::var("EMAIL_USER_ACCOUNT")
        .context("请在 .env 中设置 EMAIL_USER_ACCOUNT（jAccount 账号或交大邮箱地址）")?;
    let password = std::env::var("EMAIL_USER_PASSWORD")
        .context("请在 .env 中设置 EMAIL_USER_PASSWORD（jAccount 登录密码）")?;
    anyhow::ensure!(
        !account.trim().is_empty() && !password.is_empty(),
        "EMAIL_USER_ACCOUNT 和 EMAIL_USER_PASSWORD 不能为空"
    );
    Ok((account, password))
}

fn state_dir() -> anyhow::Result<PathBuf> {
    let path = std::env::var_os("AGENT_SSO_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(".sso"));
    if path.is_absolute() {
        Ok(path)
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}

async fn run_helper(
    action: &str,
    account: String,
    password: String,
    url: &str,
) -> anyhow::Result<HelperResult> {
    let _guard = BROWSER_LOCK.get_or_init(|| Mutex::new(())).lock().await;
    let python = std::env::var_os("AGENT_SSO_PYTHON").unwrap_or_else(|| "python3".into());
    let request = json!({
        "action": action,
        "account": account,
        "password": password,
        "url": url,
        "directory": state_dir()?,
        "browser": std::env::var("AGENT_SSO_BROWSER").ok(),
        "process_guard": PROCESS_GUARD,
        "headless": action != "open"
            && std::env::var("AGENT_SSO_HEADLESS").as_deref() != Ok("0"),
    });
    let mut command = Command::new(python);
    command
        .args(["-c", BROWSER_HELPER])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    // Credentials travel over stdin, never command-line arguments or browser environment.
    for key in [
        "EMAIL_USER_ACCOUNT",
        "EMAIL_USER_PASSWORD",
        "OPENAI_API_KEY",
        "CANVAS_API_TOKEN",
        "IMAP_PASSWORD",
    ] {
        command.env_remove(key);
    }
    let mut child = command
        .spawn()
        .context("无法启动本地 SSO 助手；请安装 Python 3，或用 AGENT_SSO_PYTHON 指定解释器")?;
    let mut stdin = child.stdin.take().context("无法打开 SSO 助手输入通道")?;
    stdin.write_all(&serde_json::to_vec(&request)?).await?;
    drop(stdin);
    let output = tokio::time::timeout(Duration::from_secs(100), child.wait_with_output())
        .await
        .context("SSO 登录超时；可稍后调用 sso_login 重试")??;
    // Do not forward Python tracebacks, browser output, cookies, or credential-bearing input.
    let result: HelperResult = serde_json::from_slice(&output.stdout)
        .context("SSO 助手未返回有效结果；请检查 Python 依赖与浏览器安装")?;
    if !result.ok {
        bail!(
            "{}",
            if result.error.is_empty() {
                "SSO 操作失败"
            } else {
                &result.error
            }
        );
    }
    anyhow::ensure!(output.status.success(), "SSO 助手异常退出");
    Ok(result)
}

pub async fn login(arguments: Value) -> Result<String, String> {
    if !arguments
        .as_object()
        .is_some_and(|object| object.is_empty())
    {
        return Err("sso_login 不接受参数，账号密码由程序从 .env 读取".into());
    }
    let (account, password) = credentials().map_err(|error| error.to_string())?;
    let result = run_helper("login", account, password, "")
        .await
        .map_err(|error| format!("{error:#}"))?;
    Ok(if result.reused {
        "已确认 jAccount 登录仍有效，登录检查页已关闭。校园网页工具将复用已保存的登录状态。"
    } else {
        "jAccount 登录成功，登录页已关闭，登录状态已保存。校园网页工具将复用已保存的登录状态。"
    }
    .into())
}

pub async fn startup() {
    if std::env::var("AGENT_SSO_AUTO_LOGIN").as_deref() == Ok("0") {
        return;
    }
    println!("[sso_login] 正在检查 jAccount 登录状态…");
    match login(json!({})).await {
        Ok(message) => println!("[sso_login] {message}"),
        Err(message) => eprintln!("[sso_login] {message}；Agent 将继续启动，可稍后重试。"),
    }
}

pub fn is_campus_url(url: &str) -> bool {
    reqwest::Url::parse(url).ok().is_some_and(|url| {
        matches!(url.scheme(), "http" | "https")
            && url
                .host_str()
                .is_some_and(|host| host == "sjtu.edu.cn" || host.ends_with(".sjtu.edu.cn"))
            && url.username().is_empty()
            && url.password().is_none()
    })
}

pub async fn visit_site(url: &str, show: bool) -> Result<String, String> {
    let valid = reqwest::Url::parse(url).ok().is_some_and(|url| {
        matches!(url.scheme(), "http" | "https")
            && url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none()
    });
    if !valid {
        return Err("网页地址必须是 HTTP(S) URL，且不能包含账号密码".into());
    }
    // Public websites use a separate anonymous profile with no campus credentials.
    let account = if is_campus_url(url) {
        std::env::var("EMAIL_USER_ACCOUNT").unwrap_or_default()
    } else {
        String::new()
    };
    let action = if show { "open" } else { "read" };
    let result = run_helper(action, account, String::new(), url)
        .await
        .map_err(|error| format!("{error:#}"))?;
    if show {
        return Ok("已打开网页供你查看，该页面将保留。".into());
    }
    let page = result.page.ok_or("浏览器未返回网页内容")?;
    if !result.page_closed {
        return Err("网页已读取，但临时页面未能自动关闭".into());
    }
    Ok(json!({"page": page, "background": result.headless, "temporary_page_closed": true}).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn campus_urls_cannot_smuggle_credentials_or_another_host() {
        assert!(is_campus_url(
            "https://i.sjtu.edu.cn/xtgl/login_slogin.html"
        ));
        assert!(is_campus_url("https://shuiyuan.sjtu.edu.cn"));
        for url in [
            "https://sjtu.edu.cn.evil.test",
            "https://evilsjtu.edu.cn",
            "https://sjtu.edu.cn@evil.test",
            "file:///sjtu.edu.cn",
            "https://user:pass@i.sjtu.edu.cn",
        ] {
            assert!(!is_campus_url(url));
        }
    }

    #[tokio::test]
    async fn model_cannot_supply_credentials_or_override_login_destination() {
        for arguments in [
            json!({"password": "secret"}),
            json!({"url": "https://example.com"}),
            Value::Null,
        ] {
            assert!(login(arguments).await.unwrap_err().contains("不接受参数"));
        }
    }
}
