use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_openai::Client;
use async_openai::types::chat::{
    ChatCompletionRequestMessage, ChatCompletionRequestSystemMessageArgs,
};
use axum::Router;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::IntoResponse;
use axum::routing::get;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;
use tokio::sync::mpsc::{self};
use tower_http::services::ServeDir;

use crate::llm::complete::{ChatEvent, chat_once};
use crate::skills;

pub fn system_prompt() -> String {
    let mut prompt = "
                你是一个上海交通大学校园信息查询助手兼学习工作小助手，名字是“交我集”，自称“小集”
                *工作规则*
                1.始终把自己当作'小集'，性格平稳，除非用户显式指定你的角色和说话方式，否则不许改变
                *输出规则*
                - 禁止使用 LaTeX，不要出现 $...$、\\frac、\\varepsilon、\\mathrm 这类写法
                - 数学式用单行 Unicode：V(r) = Q₁/(4πε₀) · (1/r − 1/R₂)
                - 分式写成 a/b 或 (a)/(b)，括号该加就加
                - 上下标用 ₀₁₂₃ 和 ⁰¹²，希腊字母直接写 ε π ρ λ Ω
                - 多行推导每行两个空格缩进，不要用 \\[ \\] 块
                "
    .to_string();
    prompt.push_str(&skills::prompt_section());
    prompt
}

pub fn new_messages() -> Vec<ChatCompletionRequestMessage> {
    let mut messages: Vec<ChatCompletionRequestMessage> = vec![];
    messages.push(
        ChatCompletionRequestSystemMessageArgs::default()
            .content(system_prompt())
            .build()
            .unwrap()
            .into(),
    );
    messages
}

pub fn router(frontend_dir: PathBuf) -> Router {
    Router::new()
        .route("/ws", get(ws_handler))
        .fallback_service(ServeDir::new(frontend_dir))
}

async fn ws_handler(ws: WebSocketUpgrade) -> impl IntoResponse {
    ws.on_upgrade(handle_socket)
}

async fn handle_socket(mut socket: WebSocket) {
    let (tx, mut rx) = mpsc::unbounded_channel::<ChatEvent>();
    let messages = Arc::new(Mutex::new(new_messages()));
    let client = Client::new();
    let busy = Arc::new(AtomicBool::new(false));

    loop {
        tokio::select! {
            Some(event) = rx.recv() => {
                let payload = match serde_json::to_string(&event) {
                    Ok(payload) => payload,
                    Err(err) => {
                        eprintln!("事件序列化失败: {err}");
                        continue;
                    }
                };
                if socket.send(Message::Text(payload.into())).await.is_err() {
                    break;
                }
            }
            incoming = socket.recv() => {
                let raw = match incoming {
                    Some(Ok(Message::Text(text))) => text.as_str().to_string(),
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(_)) => continue,
                    Some(Err(err)) => {
                        eprintln!("WebSocket 读取失败: {err}");
                        break;
                    }
                };

                let prompt = match serde_json::from_str::<serde_json::Value>(&raw) {
                    Ok(value) => value["text"].as_str().unwrap_or_default().trim().to_string(),
                    Err(err) => {
                        let _ = tx.send(ChatEvent::Error {
                            text: format!("消息格式错误: {err}"),
                        });
                        continue;
                    }
                };
                if prompt.is_empty() {
                    continue;
                }
                if prompt == "/quit" {
                    break;
                }
                if busy.swap(true, Ordering::SeqCst) {
                    let _ = tx.send(ChatEvent::Error {
                        text: "上一轮还在处理，请稍等".to_string(),
                    });
                    continue;
                }

                println!("[ws] user: {prompt}");
                let messages = messages.clone();
                let client = client.clone();
                let tx = tx.clone();
                let busy = busy.clone();
                tokio::spawn(async move {
                    let mut guard = messages.lock().await;
                    let outcome = chat_once(&mut guard, &prompt, &client, &tx).await;
                    busy.store(false, Ordering::SeqCst);
                    if let Err(err) = outcome {
                        eprintln!("本轮处理失败: {err:#}");
                        let _ = tx.send(ChatEvent::Error {
                            text: format!("{err:#}"),
                        });
                    }
                });
            }
        }
    }

    println!("[ws] 客户端已断开");
}

pub async fn serve_tcp(listener: TcpListener) {
    loop {
        let (stream, addr) = match listener.accept().await {
            Ok(pair) => pair,
            Err(err) => {
                eprintln!("接受连接失败: {err}");
                continue;
            }
        };
        tokio::spawn(handle_tcp(stream, addr));
    }
}

async fn handle_tcp(mut stream: TcpStream, addr: SocketAddr) {
    println!("[tcp] 客户端 {addr} 已连接");
    let mut messages = new_messages();
    let client = Client::new();
    let mut buffer = [0u8; 4096];
    let mut prompt = String::new();

    loop {
        let _ = stream.write_all("you:".as_bytes()).await;
        let n = match stream.read(&mut buffer).await {
            Ok(0) => {
                println!("客户端 {addr} 已断开");
                break;
            }
            Ok(n) => n,
            Err(err) => {
                eprintln!("读取客户端失败: {err}");
                break;
            }
        };

        prompt.clear();
        prompt.push_str(&String::from_utf8_lossy(&buffer[..n]));
        println!("user {addr} : {prompt}");

        if prompt.trim() == "/quit" {
            break;
        }

        let (tx, mut rx) = mpsc::unbounded_channel::<ChatEvent>();
        let run = chat_once(&mut messages, &prompt, &client, &tx);
        tokio::pin!(run);

        let outcome = loop {
            tokio::select! {
                result = &mut run => {
                    // 事件是发完就结束的，模型答完后要把队列里剩下的都吐出来
                    while let Ok(event) = rx.try_recv() {
                        forward_tcp(&mut stream, event).await;
                    }
                    break result;
                }
                Some(event) = rx.recv() => forward_tcp(&mut stream, event).await,
            }
        };

        if let Err(err) = outcome {
            eprintln!("本轮处理失败: {err:#}");
            let _ = stream
                .write_all(format!("error: {err:#}\n").as_bytes())
                .await;
        }
    }
}

async fn forward_tcp(stream: &mut TcpStream, event: ChatEvent) {
    match event {
        ChatEvent::Answer { text } => {
            let _ = stream.write_all("model:".as_bytes()).await;
            let _ = stream.write_all(text.as_bytes()).await;
            let _ = stream.write_all("\n".as_bytes()).await;
        }
        ChatEvent::Error { text } => {
            let _ = stream
                .write_all(format!("error: {text}\n").as_bytes())
                .await;
        }
        other => println!("[tcp] {other:?}"),
    }
}
