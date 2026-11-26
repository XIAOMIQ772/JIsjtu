mod doctor;
mod llm;
mod server;
mod sessions;
mod skills;
mod sso;
mod tui;

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::PathBuf;

use server::{router, serve_tcp};

fn env_port(key: &str, default: u16) -> u16 {
    std::env::var(key)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn frontend_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("AGENT_FRONTEND_DIR") {
        return PathBuf::from(dir);
    }
    if PathBuf::from("agent_frontend").is_dir() {
        return PathBuf::from("agent_frontend");
    }
    PathBuf::from("../agent_frontend")
}

const HOMEWORK_CLI_SESSION: &str = "--tool homework";

#[tokio::main(worker_threads = 4)]
async fn main() {
    if let Err(error) = dotenvy::dotenv() {
        eprintln!("提示：未加载 .env（{error}），将继续使用进程环境变量");
    }

    // 调试入口：不经过模型直接调用某个工具，例如
    // JIsjtu --tool get_time_stamp
    // JIsjtu --tool get_exam '{"course_id":"12345"}'
    let arguments: Vec<String> = std::env::args().collect();
    if let Some(index) = arguments.iter().position(|argument| argument == "--tool") {
        let Some(name) = arguments.get(index + 1) else {
            eprintln!("用法：--tool <工具名> ['<JSON 参数>']");
            std::process::exit(2);
        };
        let input = arguments.get(index + 2).map(String::as_str).unwrap_or("{}");
        let call = llm::complete::tool_calling(name, input);
        let result = if name == "homework" {
            llm::complete::tools_completion::outbound::SESSION.scope(HOMEWORK_CLI_SESSION.into(), call).await
        } else {
            call.await
        };
        match result {
            Ok(output) => println!("{output}"),
            Err(error) => {
                eprintln!("{error:#}");
                std::process::exit(1);
            }
        }
        return;
    }

    if std::env::args().any(|argument| argument == "--doctor") {
        let healthy = doctor::run().await;
        std::process::exit(if healthy { 0 } else { 1 });
    }

    if std::env::args().any(|argument| argument == "--sso-login") {
        match sso::login(serde_json::json!({})).await {
            Ok(message) => println!("{message}"),
            Err(message) => {
                eprintln!("{message}");
                std::process::exit(1);
            }
        }
        return;
    }

    let tui_mode = !std::env::args()
        .skip(1)
        .any(|argument| matches!(argument.as_str(), "--web" | "web"));
    let http_port = env_port("AGENT_HTTP_PORT", 13376);
    let tcp_port = env_port("AGENT_TCP_PORT", 8081);
    let bind_ip: IpAddr = match std::env::var_os("AGENT_BIND_ADDR") {
        None => Ipv4Addr::LOCALHOST.into(),
        Some(value) => value.to_str().and_then(|value| value.trim().parse().ok()).unwrap_or_else(|| {
            eprintln!("AGENT_BIND_ADDR 必须是一个 IPv4 或 IPv6 地址，例如 127.0.0.1");
            std::process::exit(2);
        }),
    };

    sso::startup().await;

    let frontend = frontend_dir();
    if !frontend.is_dir() {
        eprintln!(
            "警告：前端目录 {} 不存在，页面将返回 404",
            frontend.display()
        );
    }

    let skills_dir = skills::dir();
    let loaded = skills::all();
    if !skills_dir.is_dir() {
        eprintln!(
            "警告：技能目录 {} 不存在，本次不加载任何技能",
            skills_dir.display()
        );
        if std::env::var("AGENT_SKILLS_DIR").is_ok() {
            eprintln!("      （该路径来自 AGENT_SKILLS_DIR，按当前工作目录解析）");
        }
    } else if loaded.is_empty() {
        println!("技能目录 {} 中没有技能", skills_dir.display());
    } else {
        println!(
            "已从 {} 加载 {} 个技能：{}",
            skills_dir.display(),
            loaded.len(),
            loaded
                .iter()
                .map(|skill| skill.name.as_str())
                .collect::<Vec<_>>()
                .join("、")
        );
    }

    let http_listener = tokio::net::TcpListener::bind((bind_ip, http_port))
        .await
        .unwrap_or_else(|err| panic!("HTTP 地址 {} 绑定失败: {err}", SocketAddr::new(bind_ip, http_port)));
    let tcp_listener = tokio::net::TcpListener::bind((bind_ip, tcp_port))
        .await
        .unwrap_or_else(|err| panic!("TCP 地址 {} 绑定失败: {err}", SocketAddr::new(bind_ip, tcp_port)));
    let connect_ip = match bind_ip {
        IpAddr::V4(ip) if ip.is_unspecified() => IpAddr::V4(Ipv4Addr::LOCALHOST),
        IpAddr::V6(ip) if ip.is_unspecified() => IpAddr::V6(Ipv6Addr::LOCALHOST),
        ip => ip,
    };
    let http_addr = SocketAddr::new(connect_ip, http_listener.local_addr().unwrap().port());
    let url = format!("http://{http_addr}");

    if !tui_mode {
        println!("网页对话界面：{url}");
        println!("裸 TCP 接口：  nc {connect_ip} {}", tcp_listener.local_addr().unwrap().port());
    }

    let http_task = tokio::spawn(async move {
        if let Err(err) = axum::serve(http_listener, router(frontend)).await {
            eprintln!("HTTP 服务退出: {err}");
        }
    });
    let tcp_task = tokio::spawn(serve_tcp(tcp_listener));

    #[cfg(any(target_os = "macos", windows))]
    {
        llm::complete::tools_completion::attendance_watch::start();
        llm::complete::tools_completion::reminders::start();
    }

    if tui_mode {
        let result = tui::run(http_addr).await;
        http_task.abort();
        tcp_task.abort();
        let _ = http_task.await;
        let _ = tcp_task.await;
        if let Err(error) = result {
            eprintln!("TUI 退出: {error:#}");
        }
        return;
    }

    if std::env::var("AGENT_NO_BROWSER").is_ok() {
        println!("已跳过自动打开浏览器（AGENT_NO_BROWSER 已设置）");
    } else if let Err(err) = webbrowser::open(&url) {
        eprintln!("自动打开浏览器失败（请手动访问 {url}）: {err}");
    }

    std::future::pending::<()>().await;
}
