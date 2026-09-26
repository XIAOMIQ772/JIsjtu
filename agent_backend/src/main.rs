mod llm;
mod server;
mod sessions;
mod skills;
mod tui;

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

#[tokio::main(worker_threads = 4)]
async fn main() {
    if let Err(error) = dotenvy::dotenv() {
        eprintln!("提示：未加载 .env（{error}），将继续使用进程环境变量");
    }

    let tui_mode = std::env::args().any(|argument| argument == "--tui");
    let http_port = env_port("AGENT_HTTP_PORT", 13376);
    let tcp_port = env_port("AGENT_TCP_PORT", 8081);

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

    let http_listener = tokio::net::TcpListener::bind(("0.0.0.0", http_port))
        .await
        .unwrap_or_else(|err| panic!("HTTP 端口 {http_port} 绑定失败: {err}"));
    let tcp_listener = tokio::net::TcpListener::bind(("0.0.0.0", tcp_port))
        .await
        .unwrap_or_else(|err| panic!("TCP 端口 {tcp_port} 绑定失败: {err}"));

    println!("网页对话界面：http://127.0.0.1:{http_port}");
    println!("裸 TCP 接口：  nc 127.0.0.1 {tcp_port}");

    let http_task = tokio::spawn(async move {
        if let Err(err) = axum::serve(http_listener, router(frontend)).await {
            eprintln!("HTTP 服务退出: {err}");
        }
    });
    let tcp_task = tokio::spawn(serve_tcp(tcp_listener));

    if tui_mode {
        let result = tui::run(http_port).await;
        http_task.abort();
        tcp_task.abort();
        let _ = http_task.await;
        let _ = tcp_task.await;
        if let Err(error) = result {
            eprintln!("TUI 退出: {error:#}");
        }
        return;
    }

    let url = format!("http://127.0.0.1:{http_port}");
    if std::env::var("AGENT_NO_BROWSER").is_ok() {
        println!("已跳过自动打开浏览器（AGENT_NO_BROWSER 已设置）");
    } else if let Err(err) = webbrowser::open(&url) {
        eprintln!("自动打开浏览器失败（请手动访问 {url}）: {err}");
    }

    std::future::pending::<()>().await;
}
