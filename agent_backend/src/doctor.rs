//! `--doctor`：逐项检查配置是否可用，并说明怎么修。网络检查并发执行，各自限时。

use std::future::Future;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Debug, PartialEq, Eq)]
pub enum Level {
    Ok,
    Warn,
    Fail,
}

pub struct Check {
    pub level: Level,
    pub title: &'static str,
    pub detail: String,
}

fn check(level: Level, title: &'static str, detail: impl Into<String>) -> Check {
    Check { level, title, detail: detail.into() }
}

fn env(key: &str) -> Option<String> {
    std::env::var(key).ok().map(|value| value.trim().to_string()).filter(|value| !value.is_empty())
}

async fn limited<T>(future: impl Future<Output = T>) -> Option<T> {
    tokio::time::timeout(Duration::from_secs(30), future).await.ok()
}

async fn model() -> Check {
    let (Some(base), Some(key), Some(model)) = (env("OPENAI_BASE_URL"), env("OPENAI_API_KEY"), env("MODEL")) else {
        return check(Level::Fail, "模型服务", "请在 .env 填写 OPENAI_BASE_URL、OPENAI_API_KEY、MODEL，否则无法对话");
    };
    let host = reqwest::Url::parse(&base)
        .ok()
        .and_then(|url| url.host_str().map(str::to_string))
        .unwrap_or_else(|| base.clone());
    let request = reqwest::Client::new()
        .get(format!("{}/models", base.trim_end_matches('/')))
        .bearer_auth(key)
        .send();
    match limited(request).await {
        Some(Ok(response)) if response.status().is_success() => {
            check(Level::Ok, "模型服务", format!("{host} · {model}，接口可访问"))
        }
        Some(Ok(response)) if response.status().as_u16() == 401 => {
            check(Level::Fail, "模型服务", format!("{host} 拒绝了 OPENAI_API_KEY（401），请检查密钥"))
        }
        // 有的兼容服务不提供 /models，不代表对话不可用
        Some(Ok(response)) => check(Level::Warn, "模型服务", format!("{host} · {model}，/models 返回 {}，对话可能仍可用", response.status())),
        Some(Err(error)) => check(Level::Fail, "模型服务", format!("无法连接 {host}：{error}")),
        None => check(Level::Fail, "模型服务", format!("连接 {host} 超时")),
    }
}

async fn canvas() -> Check {
    if env("CANVAS_API_TOKEN").is_none() {
        return check(Level::Warn, "Canvas", "未配置 CANVAS_API_TOKEN：课程、作业、DDL、公告、平时成绩功能不可用。在 oc.sjtu.edu.cn「账户 → 设置」生成令牌");
    }
    match limited(crate::llm::complete::tool_calling("get_courses", "{}")).await {
        Some(Ok(text)) => {
            let summary = text.lines().next().unwrap_or_default().trim_end_matches('：').to_string();
            check(Level::Ok, "Canvas", format!("令牌有效，{summary}"))
        }
        Some(Err(error)) => check(Level::Fail, "Canvas", format!("{error:#}")),
        None => check(Level::Fail, "Canvas", "请求超时"),
    }
}

async fn mail() -> Check {
    let Some(host) = env("IMAP_HOST") else {
        return check(Level::Warn, "邮箱", "未配置 IMAP_HOST：邮件功能不可用。交大邮箱可填 imap.sjtu.edu.cn，端口 993");
    };
    match limited(crate::llm::complete::tool_calling("mail_fetch", r#"{"action":"folders"}"#)).await {
        Some(Ok(_)) => check(Level::Ok, "邮箱", format!("{host} 登录成功")),
        Some(Err(error)) => check(Level::Fail, "邮箱", format!("{error:#}")),
        None => check(Level::Fail, "邮箱", "连接超时"),
    }
}

async fn python() -> Check {
    let python = env("AGENT_SSO_PYTHON").unwrap_or_else(|| "python3".into());
    let probe = tokio::process::Command::new(&python)
        .args(["-c", "import sys, websockets, ddddocr; print(sys.executable)"])
        .stdin(std::process::Stdio::null())
        .output();
    match limited(probe).await {
        Some(Ok(output)) if output.status.success() => {
            let path = String::from_utf8_lossy(&output.stdout).lines().last().unwrap_or_default().to_string();
            check(Level::Ok, "Python 依赖", format!("{path}（websockets、ddddocr 可用）"))
        }
        Some(Ok(output)) => {
            let error = String::from_utf8_lossy(&output.stderr);
            let missing = error
                .lines()
                .rev()
                .find(|line| line.contains("ModuleNotFoundError"))
                .unwrap_or("依赖导入失败");
            check(
                Level::Fail,
                "Python 依赖",
                format!("{python}：{missing}。请运行 {python} -m pip install -r requirements-sso.txt，或用 AGENT_SSO_PYTHON 指定装好依赖的解释器"),
            )
        }
        Some(Err(error)) => check(Level::Fail, "Python 依赖", format!("无法运行 {python}：{error}。请安装 Python 3.10+")),
        None => check(Level::Warn, "Python 依赖", "检查超时（首次导入 ddddocr 可能较慢）"),
    }
}

fn which(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|dir| dir.join(name))
            .find(|path| path.is_file())
    })
}

/// 与 sso/browser.py 的查找顺序一致：先 Edge，再 Chrome / Chromium。
fn browser() -> Check {
    if let Some(configured) = env("AGENT_SSO_BROWSER") {
        let path = which(&configured).unwrap_or_else(|| PathBuf::from(&configured));
        return if path.is_file() {
            check(Level::Ok, "浏览器", path.display().to_string())
        } else {
            check(Level::Fail, "浏览器", format!("AGENT_SSO_BROWSER 指向的 {configured} 不存在"))
        };
    }
    let fixed = [
        "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    ];
    let found = fixed
        .iter()
        .map(PathBuf::from)
        .find(|path| path.is_file())
        .or_else(|| {
            ["microsoft-edge", "microsoft-edge-stable", "msedge", "google-chrome", "google-chrome-stable", "chromium", "chromium-browser"]
                .iter()
                .find_map(|name| which(name))
        });
    match found {
        Some(path) => check(Level::Ok, "浏览器", path.display().to_string()),
        None => check(Level::Fail, "浏览器", "没有找到 Edge、Chrome 或 Chromium；校园网页、教务、水源功能需要其中之一"),
    }
}

fn jaccount() -> Check {
    if env("EMAIL_USER_ACCOUNT").is_none() || env("EMAIL_USER_PASSWORD").is_none() {
        return check(Level::Warn, "jAccount", "未配置 EMAIL_USER_ACCOUNT / EMAIL_USER_PASSWORD：教务课表、考试、成绩与水源功能不可用");
    }
    let dir = env("AGENT_SSO_DIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(".sso"));
    let saved = std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .any(|entry| entry.path().join("cookies.json").is_file());
    if saved {
        check(Level::Ok, "jAccount", format!("已有保存的登录状态（{}）；过期时会自动重新登录", dir.display()))
    } else {
        check(Level::Warn, "jAccount", format!("{} 中还没有登录状态，首次使用会自动登录，也可先运行 --sso-login", dir.display()))
    }
}

async fn calendar() -> Check {
    match crate::llm::complete::tool_calling("get_time_stamp", "{}").await {
        Ok(text) => {
            let line = text.lines().nth(1).unwrap_or_default().to_string();
            if line.contains("不在已知学期") {
                check(Level::Warn, "校历", format!("{line}。新学期开始后请在 .env 设置 SJTU_TERM_START（第 1 周周一）"))
            } else {
                check(Level::Ok, "校历", line)
            }
        }
        Err(error) => check(Level::Fail, "校历", format!("{error:#}")),
    }
}

fn directory(title: &'static str, dir: &Path, writable: bool) -> Check {
    if !dir.is_dir() {
        return check(Level::Warn, title, format!("{} 不存在", dir.display()));
    }
    if writable {
        let probe = dir.join(".jisjtu-doctor");
        let result = std::fs::write(&probe, b"ok");
        let _ = std::fs::remove_file(&probe);
        if let Err(error) = result {
            return check(Level::Fail, title, format!("{} 不可写：{error}", dir.display()));
        }
    }
    check(Level::Ok, title, dir.display().to_string())
}

pub fn render(checks: &[Check]) -> String {
    let mut text = String::from("交我集配置自检\n");
    for item in checks {
        let mark = match item.level {
            Level::Ok => "✓",
            Level::Warn => "!",
            Level::Fail => "✗",
        };
        text.push_str(&format!("{mark} {}：{}\n", item.title, item.detail));
    }
    let failed = checks.iter().filter(|item| item.level == Level::Fail).count();
    let warned = checks.iter().filter(|item| item.level == Level::Warn).count();
    text.push_str(&match (failed, warned) {
        (0, 0) => format!("共 {} 项，全部通过。", checks.len()),
        (0, warned) => format!("共 {} 项，{warned} 项提示（对应功能不可用，其余正常）。", checks.len()),
        (failed, _) => format!("共 {} 项，{failed} 项失败需要处理，{warned} 项提示。", checks.len()),
    });
    text
}

/// 返回是否没有失败项。
pub async fn run() -> bool {
    let env_file = std::env::current_dir()
        .ok()
        .and_then(|dir| dir.ancestors().map(|dir| dir.join(".env")).find(|path| path.is_file()));
    let mut checks = vec![match env_file {
        Some(path) => check(Level::Ok, "配置文件", path.display().to_string()),
        None => check(Level::Warn, "配置文件", "没有找到 .env，将只使用进程环境变量"),
    }];
    let (model, canvas, mail, python, calendar) = tokio::join!(model(), canvas(), mail(), python(), calendar());
    checks.extend([model, canvas, mail, python, browser(), jaccount(), calendar]);
    let skills = crate::skills::all();
    checks.push(match skills.len() {
        0 => check(Level::Warn, "技能", format!("{} 中没有技能", crate::skills::dir().display())),
        count => check(Level::Ok, "技能", format!("{count} 个（{}）", crate::skills::dir().display())),
    });
    checks.push(directory("会话目录", &crate::sessions::dir(), true));
    println!("{}", render(&checks));
    checks.iter().all(|item| item.level != Level::Fail)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_counts_failures_and_warnings() {
        let checks = vec![
            check(Level::Ok, "模型服务", "可访问"),
            check(Level::Warn, "邮箱", "未配置"),
            check(Level::Fail, "浏览器", "没有找到"),
        ];
        let text = render(&checks);
        assert!(text.contains("✓ 模型服务：可访问\n! 邮箱：未配置\n✗ 浏览器：没有找到\n"), "{text}");
        assert!(text.ends_with("共 3 项，1 项失败需要处理，1 项提示。"), "{text}");
        assert!(render(&checks[..1]).ends_with("全部通过。"));
    }

    #[test]
    fn directory_check_reports_missing_and_writable() {
        let missing = directory("会话目录", Path::new("/definitely/not/here"), true);
        assert_eq!(missing.level, Level::Warn);
        let temp = std::env::temp_dir();
        assert_eq!(directory("会话目录", &temp, true).level, Level::Ok);
        assert!(!temp.join(".jisjtu-doctor").exists(), "探测文件要删掉");
    }
}
