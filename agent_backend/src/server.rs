use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use async_openai::Client;
use async_openai::types::chat::{
    ChatCompletionRequestMessage, ChatCompletionRequestSystemMessageArgs,
};
use axum::Router;
use axum::extract::Path as AxumPath;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::response::Json;
use axum::routing::get;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tower_http::services::ServeDir;

use crate::llm::complete::{ChatEvent, chat_once};
use crate::sessions::{self, Store};
use crate::skills;

pub fn system_prompt() -> String {
    let mut prompt = "
                你是一个上海交通大学校园信息查询助手兼学习工作小助手，名字是“交我集”，自称“小集”
                *工作规则*
                1.始终把自己当作'小集'，性格平稳，除非用户显式指定你的角色和说话方式，否则不许改变
                2.会话的创建、时间戳、历史消息与工具调用由服务端自动保存，无需自行创建、重命名或修改 sessions 会话文件。

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
    let store = Arc::new(Store::new(sessions::dir()));
    Router::new()
        .route("/ws", get(ws_handler))
        .route("/api/sessions", get(list_sessions).post(new_session))
        .route("/api/sessions/{id}", get(get_session))
        .with_state(store)
        .fallback_service(ServeDir::new(frontend_dir))
        .layer(axum::middleware::map_response(|mut response: axum::response::Response| async move {
            response.headers_mut().insert(
                axum::http::header::CACHE_CONTROL,
                axum::http::HeaderValue::from_static("no-store"),
            );
            response
        }))
}

async fn list_sessions(
    axum::extract::State(store): axum::extract::State<Arc<Store>>,
) -> Result<Json<Vec<serde_json::Value>>, (StatusCode, String)> {
    store.list().await.map(Json).map_err(internal_error)
}

async fn new_session(
    axum::extract::State(store): axum::extract::State<Arc<Store>>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let session = store.create().await.map_err(internal_error)?;
    Ok(Json(session.lock().await.summary()))
}

async fn get_session(
    axum::extract::State(store): axum::extract::State<Arc<Store>>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let session = store.get(&id).await.map_err(internal_error)?;
    Ok(Json(session.lock().await.snapshot()))
}

fn internal_error(error: anyhow::Error) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    axum::extract::State(store): axum::extract::State<Arc<Store>>,
    axum::extract::Query(query): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, store, query.get("session_id").cloned()))
}

async fn handle_socket(mut socket: WebSocket, store: Arc<Store>, id: Option<String>) {
    let result = match id {
        Some(id) => store.get(&id).await,
        None => store.create().await,
    };
    let mut session = match result {
        Ok(session) => session,
        Err(error) => {
            let _ = socket
                .send(Message::Text(
                    serde_json::json!({"type":"session_error", "text":error.to_string()})
                        .to_string()
                        .into(),
                ))
                .await;
            return;
        }
    };
    let (mut session_events, snapshot) = {
        let guard = session.lock().await;
        (guard.tx.subscribe(), guard.snapshot())
    };
    if socket
        .send(Message::Text(snapshot.to_string().into()))
        .await
        .is_err()
    {
        return;
    }

    loop {
        tokio::select! {
            event = session_events.recv() => {
                let payload = match event {
                    Ok(payload) => payload,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => session.lock().await.snapshot(),
                    Err(_) => break,
                };
                if socket.send(Message::Text(payload.to_string().into())).await.is_err() { break; }
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

                let value = match serde_json::from_str::<serde_json::Value>(&raw) {
                    Ok(value) => value,
                    Err(err) => {
                        let _ = socket.send(Message::Text(serde_json::json!({"type":"request_error", "session_id": session.lock().await.record.id, "text":format!("消息格式错误: {err}")}).to_string().into())).await;
                        continue;
                    }
                };
                if value["type"].as_str() == Some("switch_session") {
                    let Some(id) = value["session_id"].as_str() else { continue; };
                    match store.get(id).await {
                        Ok(next) => {
                            let guard = next.lock().await;
                            session_events = guard.tx.subscribe();
                            let payload = guard.snapshot();
                            drop(guard);
                            if socket.send(Message::Text(payload.to_string().into())).await.is_err() { break; }
                            session = next;
                        }
                        Err(error) => { let _ = socket.send(Message::Text(serde_json::json!({"type":"session_error", "session_id":id, "text":error.to_string()}).to_string().into())).await; }
                    }
                    continue;
                }
                let prompt = value["text"].as_str().unwrap_or_default().trim().to_string();
                if prompt.is_empty() {
                    continue;
                }
                if prompt == "/quit" {
                    break;
                }
                let selected_id = session.lock().await.record.id.clone();
                let requested_id = value["session_id"].as_str().unwrap_or(&selected_id);
                let result = if requested_id != selected_id {
                    Err(anyhow::anyhow!("会话已切换，请重新发送"))
                } else { store.start(session.clone(), prompt).await };
                if let Err(error) = result {
                    let _ = socket.send(Message::Text(serde_json::json!({"type":"request_error", "session_id":requested_id, "text":error.to_string()}).to_string().into())).await;
                }
            }
        }
    }

    println!("[log] 客户端已断开");
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

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<ChatEvent>();
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
