use super::*;
use axum::response::{IntoResponse, Response, Sse, sse::Event};
use axum::{Json, Router, extract::State, http::StatusCode, routing::post};
use futures_util::StreamExt;
use serde_json::{Value, json};
use std::collections::HashSet;
use std::convert::Infallible;
use std::time::Duration;
use tokio::sync::{Barrier, Notify};

use crate::llm::complete::{ChatEvent, chat_with_tools, tools::get_tools};

fn arguments() -> Value {
    json!({
        "number": 2,
        "roles": ["资料查询", "结果核对"],
        "tasks": ["查询课程", "核对截止时间"],
        "messages": ["当前学期", null],
    })
}

#[test]
fn subagent_schema_and_nullable_context_match_runtime() {
    let tools = serde_json::to_value(get_tools()).unwrap();
    let tools = tools.as_array().unwrap();
    let names: HashSet<_> = tools
        .iter()
        .map(|tool| tool["function"]["name"].as_str().unwrap())
        .collect();
    assert_eq!(names.len(), tools.len(), "tool names must be unique");
    let tool = tools
        .iter()
        .find(|tool| tool["function"]["name"] == "create_subagents")
        .unwrap();
    let parameters = &tool["function"]["parameters"];
    assert_eq!(parameters["properties"]["number"]["type"], "integer");
    assert_eq!(parameters["properties"]["number"]["minimum"], 1);
    for name in ["roles", "tasks"] {
        assert_eq!(parameters["properties"][name]["items"]["type"], "string");
    }
    assert_eq!(
        parameters["properties"]["messages"]["items"]["type"],
        json!(["string", "null"])
    );

    let tasks = parse_subagent_tasks(arguments()).unwrap();
    assert_eq!(tasks.len(), 2);
    assert_eq!(tasks[0].role, "资料查询");
    assert_eq!(tasks[0].task, "查询课程");
    assert_eq!(tasks[0].context.as_deref(), Some("当前学期"));
    assert!(tasks[1].context.is_none());
}

#[tokio::test]
async fn invalid_subagent_arguments_fail_before_model_configuration_or_execution() {
    for (field, value, expected) in [
        ("number", json!(0), "大于 0"),
        ("number", json!(-1), "参数错误"),
        ("number", json!(1.5), "参数错误"),
        ("number", json!("2"), "参数错误"),
        ("number", json!(usize::MAX), "长度"),
        ("roles", json!(["查询"]), "roles 长度"),
        ("tasks", json!(["查询"]), "tasks 长度"),
        ("messages", json!([null]), "messages 长度"),
        ("messages", json!([null, null, null]), "messages 长度"),
        ("roles", json!(["查询", null]), "参数错误"),
        ("tasks", json!(["查询", 42]), "参数错误"),
        ("roles", json!(["查询", "  "]), "roles[1] 不能为空"),
        ("tasks", json!(["查询", ""]), "tasks[1] 不能为空"),
        ("messages", json!([{}, null]), "参数错误"),
        ("messages", Value::Null, "参数错误"),
        ("extra", json!(true), "参数错误"),
    ] {
        let mut value_to_test = arguments();
        value_to_test[field] = value;
        let error = call("create_subagents", value_to_test).await.unwrap_err();
        assert!(error.contains(expected), "{field}: {error}");
    }
    for field in ["number", "roles", "tasks", "messages"] {
        let mut value = arguments();
        value.as_object_mut().unwrap().remove(field);
        let error = call("create_subagents", value).await.unwrap_err();
        assert!(error.contains("参数错误"), "{field}: {error}");
    }
}

#[derive(Clone)]
struct MockState {
    requests: Arc<Mutex<Vec<Value>>>,
    first_requests: Arc<Barrier>,
    release: Arc<Notify>,
}

struct MockModel {
    client: Client<OpenAIConfig>,
    state: MockState,
    server: tokio::task::JoinHandle<()>,
}

impl MockModel {
    async fn start(agents: usize) -> Self {
        let state = MockState {
            requests: Arc::new(Mutex::new(Vec::new())),
            first_requests: Arc::new(Barrier::new(agents)),
            release: Arc::new(Notify::new()),
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let router = Router::new()
            .route("/v1/chat/completions", post(mock_completion))
            .with_state(state.clone());
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let client = Client::with_config(
            OpenAIConfig::new()
                .with_api_key("local-test-key")
                .with_org_id("")
                .with_project_id("")
                .with_api_base(format!("http://{address}/v1")),
        );
        Self {
            client,
            state,
            server,
        }
    }
}

impl Drop for MockModel {
    fn drop(&mut self) {
        self.server.abort();
    }
}

fn chunk(delta: Value, finish_reason: Value) -> Value {
    json!({
        "id": "mock-completion", "object": "chat.completion.chunk",
        "created": 1, "model": "mock-model",
        "choices": [{"index": 0, "finish_reason": finish_reason, "delta": delta}],
    })
}

fn sse(chunks: Vec<Value>) -> Response {
    let events = chunks
        .into_iter()
        .map(|chunk| Event::default().data(chunk.to_string()))
        .chain(std::iter::once(Event::default().data("[DONE]")))
        .map(Ok::<_, Infallible>);
    Sse::new(futures_util::stream::iter(events)).into_response()
}

fn completion(message: Value) -> Response {
    let mut chunks = vec![chunk(
        json!({"role": "assistant", "content": ""}),
        Value::Null,
    )];
    if let Some(text) = message["content"].as_str() {
        for character in text.chars() {
            chunks.push(chunk(
                json!({"content": character.to_string()}),
                Value::Null,
            ));
        }
    }
    let mut reason = "stop";
    if let Some(calls) = message["tool_calls"].as_array() {
        reason = "tool_calls";
        // Interleave calls and split their names and arguments across SSE events.
        for (index, call) in calls.iter().enumerate().rev() {
            chunks.push(chunk(json!({"tool_calls": [{
                "index": index, "id": call["id"], "type": "function",
                "function": {"name": &call["function"]["name"].as_str().unwrap()[..2], "arguments": ""},
            }]}), Value::Null));
        }
        for (index, call) in calls.iter().enumerate() {
            let name = call["function"]["name"].as_str().unwrap();
            let arguments = call["function"]["arguments"].as_str().unwrap();
            chunks.push(chunk(
                json!({"tool_calls": [{
                    "index": index, "function": {"name": &name[2..]},
                }]}),
                Value::Null,
            ));
            for part in arguments.chars() {
                chunks.push(chunk(
                    json!({"tool_calls": [{
                        "index": index, "function": {"arguments": part.to_string()},
                    }]}),
                    Value::Null,
                ));
            }
        }
    }
    chunks.push(chunk(json!({}), json!(reason)));
    let mut usage = chunk(json!({}), Value::Null);
    usage["choices"] = json!([]);
    chunks.push(usage);
    sse(chunks)
}

async fn mock_completion(State(state): State<MockState>, Json(request): Json<Value>) -> Response {
    let messages = request["messages"].as_array().unwrap();
    let task = messages
        .iter()
        .rev()
        .find(|message| message["role"] == "user")
        .unwrap()["content"]
        .as_str()
        .unwrap();
    let first_request = messages.last().unwrap()["role"] == "user";
    state.requests.lock().unwrap().push(request.clone());
    if first_request {
        // A serial implementation cannot pass this barrier.
        state.first_requests.wait().await;
    }
    if task == "stream-early" {
        let first = futures_util::stream::iter([Ok::<_, Infallible>(
            Event::default().data(chunk(json!({"content": "第一段"}), Value::Null).to_string()),
        )]);
        let rest = futures_util::stream::once(async move {
            state.release.notified().await;
            Ok::<_, Infallible>(
                Event::default()
                    .data(chunk(json!({"content": "，后续内容"}), Value::Null).to_string()),
            )
        })
        .chain(futures_util::stream::iter([
            Ok(Event::default().data(chunk(json!({}), json!("stop")).to_string())),
            Ok(Event::default().data("[DONE]")),
        ]));
        return Sse::new(first.chain(rest)).into_response();
    }
    if matches!(task, "stream-truncated" | "stream-limited") {
        let mut chunks = vec![
            chunk(json!({"content": "尚未完成的回复"}), Value::Null),
            chunk(
                json!({"tool_calls": [{
                    "index": 0, "id": "must-not-run", "type": "function",
                    "function": {"name": "get_time_stamp", "arguments": "{}"},
                }]}),
                Value::Null,
            ),
        ];
        if task == "stream-limited" {
            chunks.push(chunk(json!({}), json!("length")));
        }
        return sse(chunks);
    }
    if task == "task-2" {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": {"message": "模拟模型错误", "type": "invalid_request_error"},
            })),
        )
            .into_response();
    }
    if task == "task-3" {
        return completion(json!({"role": "assistant", "content": null}));
    }
    if task == "task-4" {
        let mut response = chunk(json!({}), Value::Null);
        response["choices"] = json!([]);
        return sse(vec![response]);
    }
    if first_request && matches!(task, "task-0" | "root-task") {
        let mut calls = vec![json!({
            "id": "time-call", "type": "function",
            "function": {"name": "get_time_stamp", "arguments": "{}"},
        })];
        if task == "task-0" {
            calls.push(json!({
                "id": "invalid-call", "type": "function",
                "function": {"name": "read", "arguments": "invalid-json"},
            }));
            calls.push(json!({
                "id": "nested-call", "type": "function",
                "function": {"name": "create_subagents", "arguments": arguments().to_string()},
            }));
        }
        return completion(json!({"role": "assistant", "content": null, "tool_calls": calls}));
    }
    completion(json!({"role": "assistant", "content": format!("完成：{task}")}))
}

#[tokio::test]
async fn subagents_run_concurrently_with_isolated_contexts_tools_and_partial_failures() {
    let mock = MockModel::start(5).await;
    let tasks = parse_subagent_tasks(json!({
        "number": 5,
        "roles": ["role-0", "role-1", "role-2", "role-3", "role-4"],
        "tasks": ["task-0", "task-1", "task-2", "task-3", "task-4"],
        "messages": ["context-0", null, "context-2", null, null],
    }))
    .unwrap();
    let result = tokio::time::timeout(
        Duration::from_secs(10),
        run_subagents(tasks, mock.client.clone(), "mock-model"),
    )
    .await
    .expect("all agents must start concurrently")
    .unwrap();
    let result: Value = serde_json::from_str(&result).unwrap();
    assert_eq!(result["number"], 5);
    let results = result["results"].as_array().unwrap();
    assert_eq!(results.len(), 5);
    for (index, result) in results.iter().enumerate() {
        assert_eq!(result["index"], index + 1);
        assert_eq!(result["role"], format!("role-{index}"));
        assert_eq!(result["task"], format!("task-{index}"));
        assert_eq!(result["ok"], index < 2);
    }
    assert_eq!(results[0]["result"], "完成：task-0");
    assert_eq!(results[1]["result"], "完成：task-1");
    assert!(
        results[2]["error"]
            .as_str()
            .unwrap()
            .contains("模拟模型错误")
    );
    assert!(
        results[3]["error"]
            .as_str()
            .unwrap()
            .contains("模型既没有文本")
    );
    assert!(
        results[4]["error"]
            .as_str()
            .unwrap()
            .contains("模型没有返回 choice")
    );

    let requests = mock.state.requests.lock().unwrap();
    assert_eq!(
        requests.len(),
        6,
        "only the tool-using child needs a second model call"
    );
    for request in requests.iter() {
        assert_eq!(request["model"], "mock-model");
        assert_eq!(request["stream"], true);
        assert!(
            !request["tools"]
                .as_array()
                .unwrap()
                .iter()
                .any(|tool| { tool["function"]["name"] == "create_subagents" })
        );
    }
    for index in 0..5 {
        let task = format!("task-{index}");
        let request = requests
            .iter()
            .find(|request| {
                let last = request["messages"].as_array().unwrap().last().unwrap();
                last["role"] == "user" && last["content"] == task
            })
            .unwrap();
        let messages = request["messages"].to_string();
        assert!(messages.contains(&format!("role-{index}")));
        for other in 0..5 {
            if other != index {
                assert!(!messages.contains(&format!("role-{other}")));
                assert!(!messages.contains(&format!("task-{other}")));
                assert!(!messages.contains(&format!("context-{other}")));
            }
        }
        if matches!(index, 0 | 2) {
            assert!(messages.contains(&format!("context-{index}")));
        }
    }
    let history = requests.last().unwrap()["messages"].as_array().unwrap();
    let assistant = history
        .iter()
        .find(|message| message["tool_calls"].is_array())
        .unwrap();
    assert_eq!(assistant["tool_calls"].as_array().unwrap().len(), 3);
    let tool_results: Vec<_> = history
        .iter()
        .filter(|message| message["role"] == "tool")
        .collect();
    assert_eq!(tool_results.len(), 3);
    assert_eq!(tool_results[0]["tool_call_id"], "time-call");
    chrono::NaiveDateTime::parse_from_str(
        tool_results[0]["content"].as_str().unwrap(),
        "%Y-%m-%d %H:%M:%S",
    )
    .unwrap();
    assert_eq!(tool_results[1]["tool_call_id"], "invalid-call");
    assert!(
        tool_results[1]["content"]
            .as_str()
            .unwrap()
            .contains("工具执行失败")
    );
    assert_eq!(tool_results[2]["tool_call_id"], "nested-call");
    assert!(
        tool_results[2]["content"]
            .as_str()
            .unwrap()
            .contains("子 agent 不能再次调用")
    );
}

#[tokio::test]
async fn shared_chat_loop_preserves_parent_events_and_history() {
    let mock = MockModel::start(1).await;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let mut messages = new_messages();
    let answer = tokio::time::timeout(
        Duration::from_secs(10),
        chat_with_tools(
            &mut messages,
            "root-task",
            &mock.client,
            "mock-model",
            Some(&tx),
            true,
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(answer, "完成：root-task");
    assert!(
        matches!(rx.try_recv().unwrap(), ChatEvent::ToolCall { name, .. } if name == "get_time_stamp")
    );
    assert!(
        matches!(rx.try_recv().unwrap(), ChatEvent::ToolResult { name, ok: true, .. } if name == "get_time_stamp")
    );
    let mut fragments = String::new();
    loop {
        match rx.try_recv().unwrap() {
            ChatEvent::AnswerDelta { text } => fragments.push_str(&text),
            ChatEvent::Answer { text } => {
                assert_eq!(text, answer);
                break;
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }
    assert_eq!(fragments, answer);
    assert!(rx.try_recv().is_err());
    let history = serde_json::to_value(messages).unwrap();
    assert_eq!(
        history.as_array().unwrap().last().unwrap()["content"],
        answer
    );
    let requests = mock.state.requests.lock().unwrap();
    assert!(
        requests[0]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tool| { tool["function"]["name"] == "create_subagents" })
    );
}

#[tokio::test]
async fn answer_fragments_arrive_before_the_model_finishes() {
    let mock = MockModel::start(1).await;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let mut messages = new_messages();
    let run = chat_with_tools(
        &mut messages,
        "stream-early",
        &mock.client,
        "mock-model",
        Some(&tx),
        true,
    );
    let observe = async {
        assert!(
            matches!(rx.recv().await.unwrap(), ChatEvent::AnswerDelta { text } if text == "第一段")
        );
        assert!(
            rx.try_recv().is_err(),
            "the final answer must still be pending"
        );
        mock.state.release.notify_one();
        assert!(
            matches!(rx.recv().await.unwrap(), ChatEvent::AnswerDelta { text } if text == "，后续内容")
        );
        assert!(
            matches!(rx.recv().await.unwrap(), ChatEvent::Answer { text } if text == "第一段，后续内容")
        );
    };
    let (answer, ()) =
        tokio::time::timeout(Duration::from_secs(5), async { tokio::join!(run, observe) })
            .await
            .unwrap();
    assert_eq!(answer.unwrap(), "第一段，后续内容");
    let history = serde_json::to_value(messages).unwrap();
    assert_eq!(
        history.as_array().unwrap().last().unwrap()["content"],
        "第一段，后续内容"
    );
}

#[tokio::test]
async fn interrupted_stream_keeps_partial_text_without_executing_tools() {
    for (prompt, expected) in [
        ("stream-truncated", "提前结束"),
        ("stream-limited", "长度限制"),
    ] {
        let mock = MockModel::start(1).await;
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut messages = new_messages();
        let error = tokio::time::timeout(
            Duration::from_secs(5),
            chat_with_tools(
                &mut messages,
                prompt,
                &mock.client,
                "mock-model",
                Some(&tx),
                true,
            ),
        )
        .await
        .unwrap()
        .unwrap_err();
        assert!(error.to_string().contains(expected), "{error}");
        assert!(
            matches!(rx.try_recv().unwrap(), ChatEvent::AnswerDelta { text } if text == "尚未完成的回复")
        );
        assert!(
            rx.try_recv().is_err(),
            "no tool call or final-answer event is allowed"
        );
        assert_eq!(mock.state.requests.lock().unwrap().len(), 1);
        let history = serde_json::to_value(messages).unwrap();
        let last = history.as_array().unwrap().last().unwrap();
        assert_eq!(last["content"], "尚未完成的回复");
        assert!(last.get("tool_calls").is_none());
    }
}
