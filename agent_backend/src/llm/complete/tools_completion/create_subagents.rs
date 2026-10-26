use async_openai::Client;
use async_openai::config::OpenAIConfig;
use async_openai::types::chat::{
    ChatCompletionRequestSystemMessageArgs, ChatCompletionRequestUserMessageArgs,
};
use std::collections::HashMap;

use tokio::task::JoinSet;

use crate::llm::complete::chat_subagent;
use crate::server::new_messages;

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateSubagentsArguments {
    number: usize,
    roles: Vec<String>,
    tasks: Vec<String>,
    messages: Vec<Option<String>>,
}

#[derive(Debug)]
pub(super) struct SubagentTask {
    pub(super) role: String,
    pub(super) task: String,
    pub(super) context: Option<String>,
}

pub(super) fn parse_subagent_tasks(arguments: serde_json::Value) -> Result<Vec<SubagentTask>, String> {
    let args: CreateSubagentsArguments = serde_json::from_value(arguments)
        .map_err(|err| format!("create_subagents 参数错误: {err}"))?;
    if args.number == 0 {
        return Err("create_subagents 的 number 必须是大于 0 的整数".to_string());
    }
    for (name, length) in [
        ("roles", args.roles.len()),
        ("tasks", args.tasks.len()),
        ("messages", args.messages.len()),
    ] {
        if length != args.number {
            return Err(format!(
                "create_subagents 的 {name} 长度为 {length}，必须与 number ({}) 一致",
                args.number
            ));
        }
    }

    let mut tasks = Vec::with_capacity(args.number);
    for (index, ((role, task), context)) in args
        .roles
        .into_iter()
        .zip(args.tasks)
        .zip(args.messages)
        .enumerate()
    {
        for (name, value) in [("roles", &role), ("tasks", &task)] {
            if value.trim().is_empty() {
                return Err(format!("create_subagents 的 {name}[{index}] 不能为空"));
            }
        }
        tasks.push(SubagentTask { role, task, context });
    }
    Ok(tasks)
}

pub(super) async fn create_subagents(arguments: serde_json::Value) -> Result<String, String> {
    // Validate every task before starting any work.
    let tasks = parse_subagent_tasks(arguments)?;
    let model = std::env::var("MODEL").map_err(|err| format!("读取 MODEL 配置失败: {err}"))?;
    run_subagents(tasks, Client::new(), &model).await
}

pub(super) async fn run_subagents(
    tasks: Vec<SubagentTask>,
    client: Client<OpenAIConfig>,
    model: &str,
) -> Result<String, String> {
    let mut results: Vec<serde_json::Value> = tasks
        .iter()
        .enumerate()
        .map(|(index, task)| {
            serde_json::json!({
                "index": index + 1,
                "role": task.role,
                "task": task.task,
            })
        })
        .collect();
    // Dropping the parent future also aborts unfinished child tasks.
    let mut pending = JoinSet::new();
    let mut task_indices = HashMap::new();
    for (index, task) in tasks.into_iter().enumerate() {
        let client = client.clone();
        let model = model.to_string();
        let handle = pending.spawn(async move { run_subagent(task, &client, &model).await });
        task_indices.insert(handle.id(), index);
    }

    while let Some(outcome) = pending.join_next_with_id().await {
        let (id, result) = match outcome {
            Ok((id, result)) => (id, result),
            Err(err) => (err.id(), Err(format!("子 agent 任务异常终止: {err}"))),
        };
        let index = task_indices
            .remove(&id)
            .ok_or_else(|| "无法匹配子 agent 的任务编号".to_string())?;
        results[index]["ok"] = serde_json::json!(result.is_ok());
        match result {
            Ok(answer) => results[index]["result"] = serde_json::json!(answer),
            Err(error) => results[index]["error"] = serde_json::json!(error),
        }
    }

    Ok(serde_json::json!({ "number": results.len(), "results": results }).to_string())
}

async fn run_subagent(
    task: SubagentTask,
    client: &Client<OpenAIConfig>,
    model: &str,
) -> Result<String, String> {
    let mut messages = new_messages();
    messages.push(
        ChatCompletionRequestSystemMessageArgs::default()
            .content(format!(
                "你是主 agent 创建的子 agent，你的角色是：{}。\n\
                 只处理分配给你的任务，必要时使用工具，不要创建其他子 agent。\n\
                 完成后向主 agent 返回具体结果，并说明无法完成的部分。",
                task.role
            ))
            .build()
            .map_err(|err| err.to_string())?
            .into(),
    );
    if let Some(context) = task.context {
        messages.push(
            ChatCompletionRequestUserMessageArgs::default()
                .content(format!("主 agent 提供的上下文：\n{context}"))
                .build()
                .map_err(|err| err.to_string())?
                .into(),
        );
    }

    chat_subagent(&mut messages, &task.task, client, model)
        .await
        .map_err(|err| format!("{err:#}"))
}
