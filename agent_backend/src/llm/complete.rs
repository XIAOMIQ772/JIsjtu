const MAX_REACT_STEPS: usize = 360;

use async_openai::types::chat::{
    ChatCompletionMessageToolCalls, ChatCompletionRequestAssistantMessageArgs,
    ChatCompletionRequestMessage, ChatCompletionRequestToolMessageArgs,
    ChatCompletionRequestUserMessageArgs, ChatCompletionTools, CreateChatCompletionRequestArgs,
};
use futures_util::future::BoxFuture;
use serde_json::Value;
use tokio::sync::mpsc::UnboundedSender;

use crate::llm::complete::tools::get_tools;
mod streaming;
pub mod tools;
pub mod tools_completion;
use streaming::{StreamedMessage, stream_message};
use tools_completion::call;

const EVENT_PREVIEW_CHARS: usize = 500;

/// 一轮对话对外抛出的进度事件。WebSocket 和裸 TCP 两个入口各自消费。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ChatEvent {
    ToolCall {
        name: String,
        arguments: String,
    },
    ToolResult {
        name: String,
        ok: bool,
        preview: String,
    },
    AnswerDelta {
        text: String,
    },
    Answer {
        text: String,
    },
    Error {
        text: String,
    },
}

fn preview(text: &str) -> String {
    let text = text.trim();
    if text.chars().count() <= EVENT_PREVIEW_CHARS {
        return text.to_string();
    }
    let head: String = text.chars().take(EVENT_PREVIEW_CHARS).collect();
    format!("{head}…")
}

/// 运行一轮 ReAct：模型决定是否调用工具，工具结果再交给模型生成最终回答。
pub async fn chat_once(
    messages: &mut Vec<ChatCompletionRequestMessage>,
    prompt: &str,
    client: &async_openai::Client<async_openai::config::OpenAIConfig>,
    tx: &UnboundedSender<ChatEvent>,
) -> anyhow::Result<()> {
    let model = std::env::var("MODEL")?;
    chat_with_tools(messages, prompt, client, &model, Some(tx), true)
        .await
        .map(|_| ())
}

// Box the future to break the chat -> tool -> subagent -> chat type cycle.
pub(super) fn chat_subagent<'a>(
    messages: &'a mut Vec<ChatCompletionRequestMessage>,
    prompt: &'a str,
    client: &'a async_openai::Client<async_openai::config::OpenAIConfig>,
    model: &'a str,
) -> BoxFuture<'a, anyhow::Result<String>> {
    Box::pin(chat_with_tools(
        messages, prompt, client, model, None, false,
    ))
}

async fn chat_with_tools(
    messages: &mut Vec<ChatCompletionRequestMessage>,
    prompt: &str,
    client: &async_openai::Client<async_openai::config::OpenAIConfig>,
    model: &str,
    tx: Option<&UnboundedSender<ChatEvent>>,
    allow_subagents: bool,
) -> anyhow::Result<String> {
    let mut tools = get_tools();
    if !allow_subagents {
        tools.retain(|tool| {
            !matches!(tool, ChatCompletionTools::Function(tool) if tool.function.name == "create_subagents")
        });
    }

    messages.push(
        ChatCompletionRequestUserMessageArgs::default()
            .content(prompt)
            .build()?
            .into(),
    );

    // 每一轮就是一次“思考 → 行动 → 观察”，步数受 MAX_REACT_STEPS 限制。
    for _ in 0..MAX_REACT_STEPS {
        let request = CreateChatCompletionRequestArgs::default()
            .model(model)
            .tools(tools.clone())
            .messages(messages.clone())
            .build()?;
        let mut message = StreamedMessage::default();
        if let Err(error) = stream_message(request, client, tx, &mut message).await {
            // Preserve visible partial text, but never execute incomplete tool calls.
            if !message.text.is_empty() {
                messages.push(
                    ChatCompletionRequestAssistantMessageArgs::default()
                        .content(message.text)
                        .build()?
                        .into(),
                );
            }
            return Err(error);
        }

        let tool_calls = message
            .tool_calls
            .into_values()
            .map(ChatCompletionMessageToolCalls::Function)
            .collect::<Vec<_>>();
        let mut assistant = ChatCompletionRequestAssistantMessageArgs::default();
        if !message.text.is_empty() {
            assistant.content(message.text.clone());
        }
        if !tool_calls.is_empty() {
            assistant.tool_calls(tool_calls.clone());
        }
        // Store one complete assistant message before its tool results.
        messages.push(assistant.build()?.into());
        if !message.text.is_empty() {
            if let Some(tx) = tx {
                let _ = tx.send(ChatEvent::Answer {
                    text: message.text.clone(),
                });
            }
        }

        if !tool_calls.is_empty() {
            for tool_call in tool_calls {
                match tool_call {
                    ChatCompletionMessageToolCalls::Function(tool) => {
                        let name = &tool.function.name;
                        let arguments = &tool.function.arguments;
                        if let Some(tx) = tx {
                            let _ = tx.send(ChatEvent::ToolCall {
                                name: name.clone(),
                                arguments: preview(arguments),
                            });
                        }

                        let outcome = if name == "create_subagents" && !allow_subagents {
                            Err(anyhow::anyhow!("子 agent 不能再次调用 create_subagents"))
                        } else {
                            tool_calling(name, arguments).await
                        };
                        let (result, ok) = match outcome {
                            Ok(result) => (result, true),
                            Err(err) => {
                                eprintln!("工具 {name} 执行失败: {err:#}");
                                (format!("工具执行失败: {err:#}"), false)
                            }
                        };

                        if let Some(tx) = tx {
                            let _ = tx.send(ChatEvent::ToolResult {
                                name: name.clone(),
                                ok,
                                preview: preview(&result),
                            });
                        }

                        // println!("工具名称：{name}");
                        // println!("工具参数：{arguments}");
                        // println!("执行结果：{result}");

                        // tool 消息必须带上对应的 tool_call_id。
                        messages.push(
                            ChatCompletionRequestToolMessageArgs::default()
                                .content(result)
                                .tool_call_id(tool.id)
                                .build()?
                                .into(),
                        );
                    }
                    ChatCompletionMessageToolCalls::Custom(_) => {
                        return Err(anyhow::anyhow!("暂不支持 custom tool"));
                    }
                }
            }

            // 下一轮请求会让模型读取刚刚加入的 tool 消息。
            continue;
        }

        return Ok(message.text);
    }

    Err(anyhow::anyhow!("ReAct 超过最大步数"))
}

pub async fn tool_calling(name: &str, arguments_json: &str) -> anyhow::Result<String> {
    let arguments: Value = serde_json::from_str(arguments_json)?;
    let result = call(name, arguments).await.map_err(anyhow::Error::msg);
    result
}
