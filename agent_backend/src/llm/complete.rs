const MAX_REACT_STEPS:usize=360;


use async_openai::types::chat::{
    ChatCompletionMessageToolCalls, ChatCompletionRequestAssistantMessageArgs,
    ChatCompletionRequestMessage, ChatCompletionRequestToolMessageArgs,
    ChatCompletionRequestUserMessageArgs, CreateChatCompletionRequestArgs,
};
use serde_json::{Value};
use tokio::sync::mpsc::UnboundedSender;

use crate::llm::complete::tools::get_tools;
pub mod tools_completion;
pub mod tools;
use tools_completion::call;

const EVENT_PREVIEW_CHARS: usize = 500;

/// 一轮对话对外抛出的进度事件。WebSocket 和裸 TCP 两个入口各自消费。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ChatEvent {
    ToolCall { name: String, arguments: String },
    ToolResult { name: String, ok: bool, preview: String },
    Answer { text: String },
    Error { text: String },
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
    messages.push(
        ChatCompletionRequestUserMessageArgs::default()
            .content(prompt)
            .build()?
            .into(),
    );

    // 每一轮就是一次“思考 → 行动 → 观察”，最多执行 8 轮，防止无限循环。
    for _ in 0..MAX_REACT_STEPS {
        let request = CreateChatCompletionRequestArgs::default()
            .model(std::env::var("MODEL")?)
            .tools(get_tools())
            .messages(messages.clone())
            .build()?;
        let response = client.chat().create(request).await?;
        let message = response
            .choices
            .first()
            .ok_or_else(|| anyhow::anyhow!("模型没有返回 choice"))?
            .message
            .clone();

        if let Some(tool_calls) = message.tool_calls.clone().filter(|calls| !calls.is_empty()) {
            // assistant 的 tool-call 消息必须先加入历史。
            messages.push(
                ChatCompletionRequestAssistantMessageArgs::default()
                    .tool_calls(tool_calls.clone())
                    .build()?
                    .into(),
            );

            for tool_call in tool_calls {
                match tool_call {
                    ChatCompletionMessageToolCalls::Function(tool) => {
                        let name = &tool.function.name;
                        let arguments = &tool.function.arguments;
                        let _ = tx.send(ChatEvent::ToolCall {
                            name: name.clone(),
                            arguments: preview(arguments),
                        });

                        let (result, ok) = match tool_calling(name, arguments).await {
                            Ok(result) => (result, true),
                            Err(err) => {
                                eprintln!("工具 {name} 执行失败: {err:#}");
                                (format!("工具执行失败: {err:#}"), false)
                            }
                        };

                        let _ = tx.send(ChatEvent::ToolResult {
                            name: name.clone(),
                            ok,
                            preview: preview(&result),
                        });

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

        let model_answer = message
            .content
            .ok_or_else(|| anyhow::anyhow!("模型既没有文本，也没有工具调用"))?;

        //println!("model : {model_answer}");
        let _ = tx.send(ChatEvent::Answer {
            text: model_answer.clone(),
        });

        messages.push(
            ChatCompletionRequestAssistantMessageArgs::default()
                .content(model_answer)
                .build()?
                .into(),
        );
        return Ok(());
    }

    Err(anyhow::anyhow!("ReAct 超过最大步数"))
}

pub async fn tool_calling(name: &str, arguments_json: &str) -> anyhow::Result<String> {
    let arguments: Value = serde_json::from_str(arguments_json)?;
    let result=call(name, arguments).await.map_err(anyhow::Error::msg);
    result
}


