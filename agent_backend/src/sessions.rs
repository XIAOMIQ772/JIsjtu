use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use async_openai::types::chat::{
    ChatCompletionRequestMessage, ChatCompletionRequestUserMessageArgs,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::{Mutex, broadcast};

#[derive(Clone, Serialize, Deserialize)]
pub struct Record {
    pub id: String,
    pub title: String,
    pub create_time: i64,
    pub updated_at: i64,
    pub messages: Vec<ChatCompletionRequestMessage>,
    pub events: Vec<Value>,
    #[serde(default)]
    pub revision: u64,
}

pub struct Session {
    pub record: Record,
    pub busy: bool,
    pub save_error: Option<String>,
    pub tx: broadcast::Sender<Value>,
}

impl Session {
    pub fn summary(&self) -> Value {
        json!({"id": self.record.id, "title": self.record.title,
            "create_time": self.record.create_time, "updated_at": self.record.updated_at,
            "busy": self.busy, "revision": self.record.revision})
    }

    pub fn snapshot(&self) -> Value {
        json!({"type": "session_snapshot", "session": self.summary(),
            "events": self.record.events, "save_error": self.save_error})
    }

    pub fn emit(&mut self, event: Value) {
        self.record.revision += 1;
        self.record.updated_at = chrono::Utc::now().timestamp_millis();
        self.record.events.push(event.clone());
        let _ = self
            .tx
            .send(json!({"type": "session_event", "session": self.summary(), "event": event}));
    }
}

pub type SharedSession = Arc<Mutex<Session>>;

pub struct Store {
    root: PathBuf,
    loaded: Mutex<HashMap<String, SharedSession>>,
}

impl Store {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            loaded: Mutex::new(HashMap::new()),
        }
    }

    pub async fn get(&self, id: &str) -> anyhow::Result<SharedSession> {
        anyhow::ensure!(valid_id(id), "无效的会话 ID");
        let mut loaded = self.loaded.lock().await;
        if let Some(session) = loaded.get(id) {
            return Ok(session.clone());
        }
        let record: Record =
            serde_json::from_slice(&std::fs::read(self.root.join(format!("{id}.json")))?)?;
        anyhow::ensure!(record.id == id, "会话 ID 与文件不一致");
        let session = wrap(record);
        loaded.insert(id.into(), session.clone());
        Ok(session)
    }

    pub async fn create(&self) -> anyhow::Result<SharedSession> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let now = chrono::Utc::now();
        let id = format!(
            "{}-{}-{}",
            now.timestamp_nanos_opt().unwrap_or_default(),
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        let session = wrap(Record {
            id: id.clone(),
            title: "新对话".into(),
            create_time: now.timestamp_millis(),
            updated_at: now.timestamp_millis(),
            messages: crate::server::new_messages(),
            events: vec![],
            revision: 0,
        });
        self.save(&session.lock().await.record)?;
        self.loaded.lock().await.insert(id, session.clone());
        Ok(session)
    }

    pub async fn list(&self) -> anyhow::Result<Vec<Value>> {
        std::fs::create_dir_all(&self.root)?;
        let ids: Vec<String> = std::fs::read_dir(&self.root)?
            .flatten()
            .filter_map(|entry| {
                let path = entry.path();
                if path.extension()?.to_str()? != "json" {
                    return None;
                }
                let id = path.file_stem()?.to_str()?;
                (id != "config" && valid_id(id)).then(|| id.to_string())
            })
            .collect();
        for id in ids {
            let _ = self.get(&id).await;
        }
        let sessions: Vec<_> = self.loaded.lock().await.values().cloned().collect();
        let mut summaries = Vec::new();
        for session in sessions {
            summaries.push(session.lock().await.summary());
        }
        summaries.sort_by_key(|s| std::cmp::Reverse(s["updated_at"].as_i64().unwrap_or(0)));
        Ok(summaries)
    }

    fn save(&self, record: &Record) -> anyhow::Result<()> {
        std::fs::create_dir_all(&self.root)?;
        let path = self.root.join(format!("{}.json", record.id));
        let stage = self.root.join(format!("{}.json.tmp", record.id));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        use std::io::Write;
        let mut file = options.open(&stage)?;
        file.write_all(&serde_json::to_vec_pretty(record)?)?;
        file.sync_all()?;
        std::fs::rename(stage, path)?;
        Ok(())
    }

    pub fn persist(&self, session: &mut Session) {
        session.save_error = self
            .save(&session.record)
            .err()
            .map(|err| format!("会话保存失败：{err}"));
    }

    pub async fn start(
        self: &Arc<Self>,
        session: SharedSession,
        prompt: String,
    ) -> anyhow::Result<()> {
        let mut guard = session.lock().await;
        anyhow::ensure!(!guard.busy, "这个会话仍在处理上一条消息");
        guard.busy = true;
        if guard.record.events.is_empty() {
            guard.record.title = prompt.chars().take(28).collect();
        }
        let mut messages = guard.record.messages.clone();
        guard.record.messages.push(
            ChatCompletionRequestUserMessageArgs::default()
                .content(prompt.clone())
                .build()?
                .into(),
        );
        guard.emit(
            json!({"type": "user", "text": prompt, "at": chrono::Utc::now().timestamp_millis()}),
        );
        // Refresh the system prompt on resume so installed skills stay current.
        if let Some(first) = messages.first_mut() {
            if matches!(first, ChatCompletionRequestMessage::System(_)) {
                *first = crate::server::new_messages().remove(0);
            }
        }
        self.persist(&mut guard);
        drop(guard);
        let store = self.clone();
        tokio::spawn(async move {
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            // Run separately so even a tool panic releases the session's busy state.
            let task = tokio::spawn(async move {
                let result = crate::llm::complete::chat_once(
                    &mut messages,
                    &prompt,
                    &async_openai::Client::new(),
                    &tx,
                )
                .await;
                (messages, result)
            });
            while let Some(event) = rx.recv().await {
                let mut value = serde_json::to_value(event).expect("serializable chat event");
                value["at"] = json!(chrono::Utc::now().timestamp_millis());
                session.lock().await.emit(value);
            }
            let mut guard = session.lock().await;
            match task.await {
                Ok((messages, result)) => {
                    guard.record.messages = messages;
                    if let Err(err) = result {
                        guard.emit(
                            serde_json::to_value(crate::llm::complete::ChatEvent::Error {
                                text: format!("{err:#}"),
                            })
                            .expect("serializable error"),
                        );
                    }
                }
                Err(err) => {
                    guard.emit(json!({"type": "error", "text": format!("任务意外中断：{err}")}))
                }
            }
            guard.busy = false;
            guard.emit(json!({"type": "done"}));
            store.persist(&mut guard);
            let _ = guard.tx.send(guard.snapshot());
        });
        Ok(())
    }
}

fn wrap(record: Record) -> SharedSession {
    let (tx, _) = broadcast::channel(256);
    Arc::new(Mutex::new(Session {
        record,
        busy: false,
        save_error: None,
        tx,
    }))
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() < 100
        && id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
}

pub fn dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("AGENT_SESSIONS_DIR") {
        return dir.into();
    }
    if Path::new("sessions").is_dir() || !Path::new("../sessions").is_dir() {
        PathBuf::from("sessions")
    } else {
        PathBuf::from("../sessions")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn isolated_sessions_survive_reload() {
        let root = std::env::temp_dir().join(format!(
            "jisjtu-sessions-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap()
        ));
        let store = Store::new(root.clone());
        let a = store.create().await.unwrap();
        let b = store.create().await.unwrap();
        let id = {
            let mut a = a.lock().await;
            a.emit(json!({"type":"user", "text":"课程"}));
            store.persist(&mut a);
            assert!(a.save_error.is_none());
            a.record.id.clone()
        };
        assert!(b.lock().await.record.events.is_empty());
        assert_eq!(store.list().await.unwrap().len(), 2);
        let reloaded = Store::new(root.clone()).get(&id).await.unwrap();
        assert_eq!(reloaded.lock().await.record.events[0]["text"], "课程");
        assert!(store.get("../config").await.is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
