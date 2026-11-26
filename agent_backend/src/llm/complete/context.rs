use std::{borrow::Cow, collections::HashSet, io};

use anyhow::{Result, ensure};
use async_openai::types::chat::{
    ChatCompletionMessageCustomToolCall, ChatCompletionMessageToolCall,
    ChatCompletionMessageToolCalls as Call, ChatCompletionRequestAssistantMessage,
    ChatCompletionRequestAssistantMessageContent as AssistantContent,
    ChatCompletionRequestMessage as Message, ChatCompletionRequestToolMessage,
    ChatCompletionRequestToolMessageContent as ToolContent,
    ChatCompletionRequestToolMessageContentPart, ChatCompletionRequestUserMessage,
    ChatCompletionRequestUserMessageContent as UserContent, CustomTool, FunctionCall,
};
use serde::Deserialize;

pub(super) fn budget() -> usize {
    std::env::var("AGENT_CONTEXT_BYTES")
        .ok()
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(96 * 1024)
}

// Count the exact escaped JSON bytes without allocating a second copy of tool output.
struct Counter(usize);

impl io::Write for Counter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0 = self
            .0
            .checked_sub(bytes.len())
            .ok_or(io::ErrorKind::WriteZero)?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

// Nonempty blocks cost one comma per message. The enclosing array costs one more byte.
fn weight(messages: &[Message], limit: usize) -> Option<usize> {
    let capacity = limit.saturating_add(1);
    let mut counter = Counter(capacity);
    serde_json::to_writer(&mut counter, messages).ok()?;
    Some(capacity - counter.0 - 1)
}

struct Block<'a> {
    messages: &'a [Message],
    size: Option<usize>,
    selected: Option<Cow<'a, [Message]>>,
}

impl Block<'_> {
    fn select(&mut self, room: &mut usize) -> bool {
        if self.selected.is_some() {
            return true;
        }
        if let Some(size) = self.size
            && size <= *room
        {
            *room -= size;
            self.selected = Some(Cow::Borrowed(self.messages));
            return true;
        }
        false
    }

    fn is_user(&self) -> bool {
        matches!(self.messages[0], Message::User(_))
    }

    fn is_preview(&self) -> bool {
        let Message::Assistant(a) = &self.messages[0] else {
            return false;
        };
        a.tool_calls.iter().flatten().any(|call| {
            let Call::Function(f) = call else {
                return false;
            };
            f.function.name == "homework"
                && self.messages[1..].iter().any(|msg| {
                    matches!(msg, Message::Tool(t) if t.tool_call_id == f.id
                    && matches!(&t.content, ToolContent::Text(text) if preview_id(text).is_some()))
                })
        })
    }
}

fn call_id(call: &Call) -> &str {
    match call {
        Call::Function(f) => &f.id,
        Call::Custom(c) => &c.id,
    }
}

// Each assistant tool call and every one of its results travel together.
// A crash can leave an incomplete group in an otherwise usable saved conversation.
#[allow(deprecated)]
fn blocks(messages: &[Message], limit: usize) -> Vec<Block<'_>> {
    let mut blocks = Vec::new();
    let mut i = 0;
    while i < messages.len() {
        let start = i;
        i += 1;
        match &messages[start] {
            Message::Assistant(a)
                if a.tool_calls.as_ref().is_some_and(|calls| !calls.is_empty()) =>
            {
                let calls = a.tool_calls.as_ref().unwrap();
                let mut pending: HashSet<_> = calls.iter().map(call_id).collect();
                let mut valid = pending.len() == calls.len();
                while let Some(Message::Tool(tool)) = messages.get(i) {
                    valid &= pending.remove(tool.tool_call_id.as_str());
                    i += 1;
                }
                if !valid || !pending.is_empty() {
                    continue;
                }
            }
            Message::Assistant(a) if a.function_call.is_some() => {
                let call = a.function_call.as_ref().unwrap();
                if !matches!(messages.get(i), Some(Message::Function(f)) if f.name == call.name) {
                    continue;
                }
                i += 1;
            }
            Message::Tool(_) | Message::Function(_) => continue,
            _ => {}
        }
        let group = &messages[start..i];
        blocks.push(Block {
            messages: group,
            size: weight(group, limit),
            selected: None,
        });
    }
    blocks
}

const CUT: &str = "…[已节选]…";

fn excerpt(text: &str, bytes: usize) -> String {
    if text.len() <= bytes {
        return text.to_owned();
    }
    let head = text.floor_char_boundary(bytes / 2);
    let tail = text.ceil_char_boundary(text.len() - bytes / 2);
    format!("{}{CUT}{}", &text[..head], &text[tail..])
}

fn preview_id(text: &str) -> Option<&str> {
    let rest = text.strip_prefix("预览已直接展示给用户。")?;
    let (_, rest) = rest.split_once("preview_id=")?;
    let id = rest.split_whitespace().next()?;
    (id.len() == 16 && id.bytes().all(|b| b.is_ascii_hexdigit())).then_some(id)
}

// Unknown fields include potentially very large drafts; serde skips their values.
#[derive(Deserialize)]
struct HomeworkTarget {
    course_id: String,
    assignment_id: String,
    action: String,
    confirm: Option<bool>,
}

fn arguments(name: &str, raw: &str, bytes: usize) -> String {
    if raw.len() <= bytes {
        return raw.to_owned();
    }
    if name == "homework"
        && let Ok(target) = serde_json::from_str::<HomeworkTarget>(raw)
    {
        return serde_json::json!({"course_id":target.course_id, "assignment_id":target.assignment_id,
            "action":target.action, "confirm":target.confirm, "_omitted": "原调用参数已节选"}).to_string();
    }
    serde_json::json!({"_excerpt": excerpt(raw, bytes)}).to_string()
}

fn tool_excerpt(content: &ToolContent, bytes: usize) -> String {
    match content {
        ToolContent::Text(text) => {
            if text.len() > bytes
                && let Some(id) = preview_id(text)
            {
                return format!("已展示作业预览，等待用户确认。preview_id={id}");
            }
            excerpt(text, bytes)
        }
        ToolContent::Array(parts) => {
            let text = |p: &ChatCompletionRequestToolMessageContentPart| match p {
                ChatCompletionRequestToolMessageContentPart::Text(t) => excerpt(&t.text, bytes / 2),
            };
            match parts.as_slice() {
                [] => String::new(),
                [p] => text(p),
                [first, .., last] => format!("{}{CUT}{}", text(first), text(last)),
            }
        }
    }
}

#[allow(deprecated)]
fn compact(group: &[Message], bytes: usize) -> Vec<Message> {
    let Message::Assistant(a) = &group[0] else {
        unreachable!("tool group starts with assistant")
    };
    let mut action = ChatCompletionRequestAssistantMessage::default();
    if let Some(calls) = &a.tool_calls {
        action.tool_calls = Some(
            calls
                .iter()
                .map(|call| match call {
                    Call::Function(f) => Call::Function(ChatCompletionMessageToolCall {
                        id: f.id.clone(),
                        function: FunctionCall {
                            name: f.function.name.clone(),
                            arguments: arguments(&f.function.name, &f.function.arguments, bytes),
                        },
                    }),
                    Call::Custom(c) => Call::Custom(ChatCompletionMessageCustomToolCall {
                        id: c.id.clone(),
                        custom_tool: CustomTool {
                            name: c.custom_tool.name.clone(),
                            input: excerpt(&c.custom_tool.input, bytes),
                        },
                    }),
                })
                .collect(),
        );
    } else if let Some(call) = &a.function_call {
        action.function_call = Some(FunctionCall {
            name: call.name.clone(),
            arguments: arguments(&call.name, &call.arguments, bytes),
        });
    }
    let mut projection = vec![Message::Assistant(action)];
    projection.extend(group[1..].iter().map(|message| match message {
        Message::Tool(t) => Message::Tool(ChatCompletionRequestToolMessage {
            tool_call_id: t.tool_call_id.clone(),
            content: ToolContent::Text(tool_excerpt(&t.content, bytes)),
        }),
        Message::Function(f) => Message::Function(
            async_openai::types::chat::ChatCompletionRequestFunctionMessage {
                name: f.name.clone(),
                content: f.content.as_ref().map(|s| excerpt(s, bytes)),
            },
        ),
        _ => unreachable!("tool group contains only results"),
    }));
    projection
}

fn collect(blocks: Vec<Block<'_>>) -> Vec<Message> {
    blocks
        .into_iter()
        .filter_map(|b| b.selected)
        .flat_map(|slice| slice.into_owned())
        .collect()
}

// "帮我缩短上一个回答" still needs the old answer when the exchange itself is too big.
fn keep_exchange(user: &mut Block<'_>, answer: &mut Block<'_>, room: &mut usize) {
    if user.selected.is_some() {
        answer.select(room);
        return;
    }
    if user
        .size
        .zip(answer.size)
        .and_then(|(a, b)| a.checked_add(b))
        .is_some_and(|size| size <= *room)
    {
        user.select(room);
        answer.select(room);
        return;
    }
    let (Message::User(u), Message::Assistant(a)) = (&user.messages[0], &answer.messages[0]) else {
        return;
    };
    let (UserContent::Text(question), Some(AssistantContent::Text(reply))) =
        (&u.content, &a.content)
    else {
        return;
    };
    let mut bytes = (*room).min(32768);
    while bytes >= 32 {
        let exchange = [
            Message::User(ChatCompletionRequestUserMessage {
                name: u.name.clone(),
                content: UserContent::Text(excerpt(question, bytes)),
            }),
            Message::Assistant(ChatCompletionRequestAssistantMessage {
                name: a.name.clone(),
                content: Some(AssistantContent::Text(excerpt(reply, bytes))),
                ..Default::default()
            }),
        ];
        if let Some(size) = weight(&exchange, *room) {
            let [question, reply] = exchange;
            user.selected = Some(Cow::Owned(vec![question]));
            answer.selected = Some(Cow::Owned(vec![reply]));
            *room -= size;
            return;
        }
        bytes /= 2;
    }
}

pub(super) fn project(messages: &[Message], budget: usize) -> Result<Vec<Message>> {
    ensure!(budget >= 2, "AGENT_CONTEXT_BYTES 至少需要 2 字节");
    let mut room = budget - 1;
    let mut groups = blocks(messages, room);
    if groups.iter().all(|g| g.size.is_some())
        && groups.iter().map(|g| g.size.unwrap() as u128).sum::<u128>() <= room as u128
    {
        return Ok(groups
            .into_iter()
            .flat_map(|g| g.messages.iter().cloned())
            .collect());
    }

    let users: Vec<_> = groups
        .iter()
        .enumerate()
        .filter(|(_, g)| g.is_user())
        .map(|(i, _)| i)
        .collect();
    let current = users.last().copied();
    for (index, group) in groups.iter_mut().enumerate() {
        if Some(index) == current
            || matches!(&group.messages[0], Message::User(u) if u.name.as_deref() == Some("subagent_context"))
            || matches!(
                group.messages[0],
                Message::System(_) | Message::Developer(_)
            )
        {
            ensure!(
                group.select(&mut room),
                "系统指令与本次用户输入超过 {budget} 字节上下文预算，请缩短输入或调大 AGENT_CONTEXT_BYTES"
            );
        }
    }

    // Reserve both the newest observation from this turn and the latest submission preview.
    let newest = groups
        .iter()
        .enumerate()
        .rfind(|(i, g)| current.is_some_and(|u| *i > u) && g.messages.len() > 1)
        .map(|(i, _)| i);
    let preview = groups.iter().rposition(Block::is_preview);
    let mut pinned = Vec::new();
    let mut reserved = 0;
    for index in [newest, preview].into_iter().flatten() {
        if pinned.iter().any(|(i, _, _)| *i == index) {
            continue;
        }
        let available = room - reserved;
        // Even the smallest excerpt keeps some head and tail from the newest observation.
        // A short, unmodified group can be smaller than the truncation labels.
        let minimum = compact(groups[index].messages, 32);
        let min_size = weight(&minimum, available)
            .into_iter()
            .chain(groups[index].size.filter(|&raw| raw <= available))
            .min();
        let Some(min_size) = min_size else {
            anyhow::bail!(
                "当前工具调用和作业预览标识超过 {budget} 字节上下文预算，请调大 AGENT_CONTEXT_BYTES"
            );
        };
        reserved += min_size;
        pinned.push((index, minimum, min_size));
    }

    for (index, minimum, min_size) in pinned {
        reserved -= min_size;
        let allowance = room - reserved;
        let group = &mut groups[index];
        if group.size.is_some_and(|size| size <= allowance) {
            group.select(&mut room);
            continue;
        }
        let per_message = (allowance / group.messages.len()).min(8192);
        let mut picked = (Cow::Owned(minimum), min_size);
        for bytes in [per_message, per_message / 2, per_message / 4, 128] {
            let slice = compact(group.messages, bytes);
            if let Some(size) = weight(&slice, allowance) {
                picked = (Cow::Owned(slice), size);
                break;
            }
        }
        room -= picked.1;
        group.selected = Some(picked.0);
    }

    // Keep recent turns intact when possible. Otherwise retain their original question
    // and final answer together, using the original roles.
    let mut end = groups.len();
    for start in users.into_iter().rev() {
        let turn = &mut groups[start..end];
        let total = turn
            .iter()
            .filter(|b| b.selected.is_none())
            .try_fold(0usize, |acc, b| acc.checked_add(b.size?));
        if total.is_some_and(|size| size <= room) {
            for block in turn {
                block.select(&mut room);
            }
        } else if Some(start) == current {
            for block in turn.iter_mut().rev() {
                block.select(&mut room);
            }
        } else if let Some(final_index) = turn
            .iter()
            .rposition(|b| b.messages.len() == 1 && matches!(b.messages[0], Message::Assistant(_)))
        {
            let (earlier, final_answer) = turn.split_at_mut(final_index);
            keep_exchange(&mut earlier[0], &mut final_answer[0], &mut room);
        }
        end = start;
    }
    let omitted: usize = groups
        .iter()
        .filter(|g| g.selected.is_none())
        .map(|g| g.messages.len())
        .sum();
    if let Some(first_gap) = groups.iter_mut().find(|g| g.selected.is_none()) {
        let notice = Message::Assistant(ChatCompletionRequestAssistantMessage {
            content: Some(AssistantContent::Text(format!(
                "【上下文整理时已省略 {omitted} 条历史消息】"
            ))),
            ..Default::default()
        });
        if weight(std::slice::from_ref(&notice), room).is_some() {
            first_gap.selected = Some(Cow::Owned(vec![notice]));
        }
    }
    Ok(collect(groups))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn msg(value: Value) -> Message {
        serde_json::from_value(value).unwrap()
    }
    fn say(role: &str, text: &str) -> Message {
        msg(json!({"role":role,"content":text}))
    }
    fn call(id: &str, name: &str, args: &str) -> Value {
        json!({"type":"function","id":id,"function":{"name":name,"arguments":args}})
    }
    fn action(calls: Vec<Value>) -> Message {
        msg(json!({"role":"assistant","tool_calls":calls}))
    }
    fn result(id: &str, text: &str) -> Message {
        msg(json!({"role":"tool","tool_call_id":id,"content":text}))
    }
    fn wire(messages: &[Message]) -> Vec<u8> {
        serde_json::to_vec(messages).unwrap()
    }

    #[test]
    fn short_input_is_exact_and_incomplete_tool_groups_are_removed() {
        let system = say("system", "规则");
        let user = say("user", "请查一查");
        let start = action(vec![call("a", "read", "{}"), call("b", "read", "{}")]);
        let good = vec![
            system.clone(),
            user.clone(),
            start.clone(),
            result("b", "第二个"),
            result("a", "第一个"),
        ];
        assert_eq!(project(&good, wire(&good).len()).unwrap(), good);
        let broken = vec![
            system.clone(),
            result("orphan", "旧记录"),
            start,
            result("a", "仅返回一次"),
            user.clone(),
        ];
        assert_eq!(project(&broken, 4000).unwrap(), vec![system, user]);
    }

    #[test]
    fn old_tool_traffic_drops_but_original_questions_and_final_answers_survive() {
        let mut messages = vec![say("system", "总规则"), say("user", "第一次查询")];
        for i in 0..94 {
            let id = i.to_string();
            messages.push(action(vec![call(&id, "get_courses", "{}")]));
            messages.push(result(&id, &"旧课程列表".repeat(100)));
        }
        let final_answer = say("assistant", "查询完成，共 5 门课。");
        let recent = vec![
            say("user", "继续"),
            action(vec![call("new", "read", "{}")]),
            result("new", "新的结果"),
            say("assistant", "第二次答复"),
        ];
        messages.push(final_answer.clone());
        messages.extend(recent.clone());
        messages.push(say("user", "结合上面的结论"));
        let projected = project(&messages, 1100).unwrap();
        let mut expected = vec![messages[0].clone(), messages[1].clone(), final_answer];
        expected.extend(recent);
        expected.push(messages.last().unwrap().clone());
        assert!(
            wire(&projected)
                .windows("上下文整理".len())
                .any(|w| w == "上下文整理".as_bytes())
        );
        let notice = say("assistant", "【上下文整理时已省略 188 条历史消息】");
        assert_eq!(
            projected
                .iter()
                .filter(|m| *m != &notice)
                .collect::<Vec<_>>(),
            expected.iter().collect::<Vec<_>>()
        );
        assert!(wire(&projected).len() <= 1100);
        assert!(wire(&messages).len() > 100_000);
    }

    #[test]
    fn a_single_long_turn_retains_the_latest_parallel_group_with_unicode_head_and_tail() {
        let mut messages = vec![
            say("system", "系统"),
            say("developer", "附加指令"),
            say("user", "完整当前需求"),
        ];
        for i in 0..40 {
            let id = i.to_string();
            messages.push(action(vec![call(&id, "read", "{}")]));
            messages.push(result(&id, &"旧".repeat(400)));
        }
        let huge = format!("开头😺{}末尾📗", "\n\"\\🧑‍🎓".repeat(1250));
        messages.push(action(vec![
            call("last1", "read", "{}"),
            call("last2", "read", "{}"),
        ]));
        messages.push(result("last2", "完整小结果"));
        messages.push(result("last1", &huge));
        let before = wire(&messages);
        let projected = project(&messages, 1500).unwrap();
        assert_eq!(before, wire(&messages));
        assert!(wire(&projected).len() <= 1500);
        assert_eq!(&projected[..3], &messages[..3]);
        let last = &projected[projected.len() - 3..];
        assert_eq!(last[0], messages[messages.len() - 3]);
        assert_eq!(last[1], messages[messages.len() - 2]);
        let Message::Tool(t) = &last[2] else { panic!() };
        let ToolContent::Text(text) = &t.content else {
            panic!()
        };
        assert_eq!(t.tool_call_id, "last1");
        assert!(text.starts_with("开头😺") && text.ends_with("末尾📗") && text.contains(CUT));
        for block in blocks(&projected, usize::MAX - 1) {
            assert!(
                block.messages.len() == 1 || matches!(block.messages[0], Message::Assistant(_))
            );
        }
    }

    #[test]
    fn latest_homework_preview_keeps_ids_even_after_another_large_tool_call() {
        let raw = json!({"action":"submit", "confirm":false, "text":"大草稿".repeat(12000),
            "course_id":"123", "assignment_id":"456"})
        .to_string();
        let text = "预览已直接展示给用户。用户同意后，以 confirm=true 和 preview_id=0123456789abcdef 确认此版本，沿用相同的 course_id、assignment_id。";
        let history = vec![
            say("system", "规则"),
            say("user", "帮我准备预览"),
            action(vec![call("preview", "homework", &raw)]),
            result("preview", text),
            say("assistant", "预览已显示"),
            say("user", "再查一下"),
            action(vec![call("recent", "read", "{}")]),
            result("recent", &"另一个大结果".repeat(4000)),
        ];
        let projected = project(&history, 1400).unwrap();
        let value = serde_json::to_value(&projected).unwrap();
        let assistant = value
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["tool_calls"][0]["id"] == "preview")
            .unwrap();
        let args: Value = serde_json::from_str(
            assistant["tool_calls"][0]["function"]["arguments"]
                .as_str()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(args["course_id"], "123");
        assert_eq!(args["assignment_id"], "456");
        assert_eq!(args["confirm"], false);
        assert!(value.to_string().contains("preview_id=0123456789abcdef"));
        assert!(value.to_string().contains("另一个大结果"));
        assert!(wire(&projected).len() <= 1400);
    }

    #[test]
    fn huge_previous_answer_and_question_can_be_revised_from_unicode_excerpts() {
        let previous_question = format!("开题🌸{}结题🌼", "春\n".repeat(4000));
        let previous_answer = format!("开始📕{}结论📘", "引文\"".repeat(9500));
        let latest = say("user", "请把上面的长答复缩短");
        let all = vec![
            say("system", "规则"),
            say("user", &previous_question),
            say("assistant", &previous_answer),
            latest.clone(),
        ];
        let projected = project(&all, 1850).unwrap();
        assert!(wire(&projected).len() <= 1850);
        assert_eq!(projected.last(), Some(&latest));
        let val = serde_json::to_value(&projected).unwrap();
        assert_eq!(val[1]["role"], "user");
        assert_eq!(val[2]["role"], "assistant");
        let q = val[1]["content"].as_str().unwrap();
        let a = val[2]["content"].as_str().unwrap();
        assert!(q.starts_with("开题🌸") && q.contains(CUT) && q.ends_with("结题🌼"));
        assert!(a.starts_with("开始📕") && a.contains(CUT) && a.ends_with("结论📘"));
    }

    #[test]
    fn exact_budget_accepts_raw_group_when_excerpt_labels_would_be_larger() {
        let latest = vec![
            say("user", "查询"),
            action(vec![call(
                "a",
                "read",
                &json!({"path":"x".repeat(30)}).to_string(),
            )]),
            result("a", "ok"),
        ];
        let mut full = vec![say("user", "过去"), say("assistant", &"很长".repeat(400))];
        full.extend(latest.clone());
        assert!(wire(&compact(&latest[1..], 32)).len() > wire(&latest[1..]).len());
        assert_eq!(project(&full, wire(&latest).len()).unwrap(), latest);
    }

    #[test]
    fn named_subagent_background_and_task_remain_complete() {
        let required = vec![
            say("system", "基本指令"),
            say("system", "子任务角色"),
            msg(json!({"role":"user", "name":"subagent_context", "content":"背景条件".repeat(40)})),
            say("user", "当前任务"),
        ];
        let mut history = vec![
            say("user", "旧请求"),
            say("assistant", &"旧答复".repeat(500)),
        ];
        history.extend(required.clone());
        let size = wire(&required).len();
        assert_eq!(project(&history, size).unwrap(), required);
        assert!(project(&history, size - 1).is_err());
    }

    #[test]
    fn shorter_followup_recovers_after_an_oversized_user_input() {
        let mut history = vec![say("system", "规则"), say("user", &"长问题".repeat(1100))];
        assert!(project(&history, 256).is_err());
        history.push(say("user", "另问一件事"));
        let recovered = project(&history, 256).unwrap();
        assert_eq!(recovered.first(), history.first());
        assert_eq!(recovered.last(), history.last());
        assert!(wire(&recovered).len() <= 256);
    }

    #[test]
    fn required_messages_are_preserved_at_the_exact_escaped_byte_boundary() {
        let system = say("system", "规则一");
        let developer = say("developer", "规则二");
        let current = say("user", &"中文🙂\n\"".repeat(20));
        let mandatory = vec![system.clone(), developer.clone(), current.clone()];
        let all = vec![
            system,
            say("user", "旧消息"),
            say("assistant", "之前的答复"),
            developer,
            current,
        ];
        let size = wire(&mandatory).len();
        assert_eq!(project(&all, size).unwrap(), mandatory);
        assert!(
            project(&all, size - 1)
                .unwrap_err()
                .to_string()
                .contains("AGENT_CONTEXT_BYTES")
        );
        assert!(project(&[], 1).is_err());
        assert!(project(&[], 2).unwrap().is_empty());
    }
}
