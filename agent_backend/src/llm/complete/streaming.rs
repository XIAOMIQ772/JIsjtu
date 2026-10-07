use std::collections::BTreeMap;

use async_openai::types::chat::{
    ChatCompletionMessageToolCall, CreateChatCompletionRequest, FinishReason,
};
use async_openai::{Client, config::OpenAIConfig};
use futures_util::StreamExt;
use tokio::sync::mpsc::UnboundedSender;

use super::ChatEvent;

#[derive(Default)]
pub(super) struct StreamedMessage {
    pub text: String,
    pub tool_calls: BTreeMap<u32, ChatCompletionMessageToolCall>,
    finish_reason: Option<FinishReason>,
    has_choice: bool,
}

pub(super) async fn stream_message(
    request: CreateChatCompletionRequest,
    client: &Client<OpenAIConfig>,
    tx: Option<&UnboundedSender<ChatEvent>>,
    message: &mut StreamedMessage,
) -> anyhow::Result<()> {
    let mut stream = client.chat().create_stream(request).await?;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        // Usage-only chunks have no choices. Only the first completion is requested.
        let Some(choice) = chunk.choices.into_iter().find(|choice| choice.index == 0) else {
            continue;
        };
        message.has_choice = true;
        for text in [choice.delta.content, choice.delta.refusal]
            .into_iter()
            .flatten()
        {
            if !text.is_empty() {
                message.text.push_str(&text);
                if let Some(tx) = tx {
                    let _ = tx.send(ChatEvent::AnswerDelta { text });
                }
            }
        }
        for delta in choice.delta.tool_calls.into_iter().flatten() {
            // Different tool calls may be interleaved across chunks.
            let tool = message.tool_calls.entry(delta.index).or_default();
            if let Some(id) = delta.id {
                tool.id.push_str(&id);
            }
            if let Some(function) = delta.function {
                if let Some(name) = function.name {
                    tool.function.name.push_str(&name);
                }
                if let Some(arguments) = function.arguments {
                    tool.function.arguments.push_str(&arguments);
                }
            }
        }
        if choice.finish_reason.is_some() {
            message.finish_reason = choice.finish_reason;
        }
    }

    anyhow::ensure!(message.has_choice, "模型没有返回 choice");
    match message.finish_reason {
        Some(FinishReason::Stop | FinishReason::ToolCalls) => {}
        Some(FinishReason::Length) => anyhow::bail!("模型输出达到长度限制，回复未完整生成"),
        Some(FinishReason::ContentFilter) => anyhow::bail!("模型输出被过滤，回复未完整生成"),
        Some(FinishReason::FunctionCall) => anyhow::bail!("暂不支持旧版 function_call"),
        None => anyhow::bail!("模型流式响应提前结束，未收到完成标记"),
    }
    anyhow::ensure!(
        !message.text.is_empty() || !message.tool_calls.is_empty(),
        "模型既没有文本，也没有工具调用"
    );
    if message.finish_reason == Some(FinishReason::ToolCalls) {
        anyhow::ensure!(!message.tool_calls.is_empty(), "模型未返回完整的工具调用");
    }
    for tool in message.tool_calls.values() {
        anyhow::ensure!(
            !tool.id.is_empty() && !tool.function.name.is_empty(),
            "模型返回的工具调用缺少 ID 或名称"
        );
    }
    Ok(())
}
