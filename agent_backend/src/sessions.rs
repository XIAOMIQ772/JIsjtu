use std::collections::HashMap;
use std::io::{Error as IoError, ErrorKind};
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
    pub deleted: bool,
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
        if self.deleted {
            return json!({"type": "session_deleted", "session_id": self.record.id});
        }
        json!({"type": "session_snapshot", "session": self.summary(),
            "events": self.record.events, "save_error": self.save_error})
    }

    pub fn emit(&mut self, event: Value) {
        self.record.revision += 1;
        self.record.updated_at = chrono::Utc::now().timestamp_millis();
        // Snapshots keep one growing answer; subscribers receive only the new fragment.
        let merged = match (self.record.events.last_mut(), event["type"].as_str()) {
            (Some(previous), Some("answer_delta" | "answer"))
                if previous["type"] == "answer_delta" =>
            {
                if event["type"] == "answer" {
                    *previous = event.clone();
                } else if let Some(Value::String(text)) = previous.get_mut("text") {
                    text.push_str(event["text"].as_str().unwrap_or_default());
                }
                true
            }
            _ => false,
        };
        if !merged {
            self.record.events.push(event.clone());
        }
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
        if !valid_id(id) {
            return Err(IoError::new(ErrorKind::InvalidInput, "无效的会话 ID").into());
        }
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
            title: "new session".into(),
            create_time: now.timestamp_millis(),
            updated_at: now.timestamp_millis(),
            messages: crate::server::new_messages(),
            events: vec![],
            revision: 0,
        });
        let mut loaded = self.loaded.lock().await;
        self.save(&session.lock().await.record)?;
        loaded.insert(id, session.clone());
        Ok(session)
    }

    pub async fn delete(&self, id: &str) -> anyhow::Result<()> {
        let session = self.get(id).await?;
        // Serialize removal with lookup and with any prompt sent over an existing socket.
        let mut loaded = self.loaded.lock().await;
        let mut session = session.lock().await;
        if session.deleted {
            return Err(IoError::new(ErrorKind::NotFound, "会话已删除").into());
        }
        if session.busy {
            return Err(IoError::new(ErrorKind::ResourceBusy, "会话正在回复，请在完成后删除").into());
        }
        // A failed or interrupted save may have left a partial copy alongside the record.
        for suffix in ["json.tmp", "json"] {
            let path = self.root.join(format!("{id}.{suffix}"));
            match std::fs::remove_file(path) {
                Err(err) if err.kind() != ErrorKind::NotFound => return Err(err.into()),
                _ => {}
            }
        }
        session.deleted = true;
        loaded.remove(id);
        let _ = session.tx.send(session.snapshot());
        Ok(())
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
                valid_id(id).then(|| id.to_string())
            })
            .collect();
        for id in ids {
            let _ = self.get(&id).await;
        }
        let sessions: Vec<_> = self.loaded.lock().await.values().cloned().collect();
        let mut summaries = Vec::new();
        for session in sessions {
            let guard = session.lock().await;
            if !guard.deleted {
                summaries.push(guard.summary());
            }
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

    fn persist(&self, session: &mut Session) {
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
        anyhow::ensure!(!guard.deleted, "这个会话已删除，请切换会话后再发送");
        anyhow::ensure!(!guard.busy, "这个会话仍在处理上一条消息");
        guard.busy = true;
        if guard.record.events.is_empty() {
            guard.record.title = prompt.chars().take(28).collect();
        }
        let mut messages = guard.record.messages.clone();
        let session_id = guard.record.id.clone();
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
                    &session_id,
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
        deleted: false,
        save_error: None,
        tx,
    }))
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && !id.eq_ignore_ascii_case("config")
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
    async fn deletion_removes_only_the_idle_history_and_invalidates_old_handles() {
        let root = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::new(root.path().into()));
        let target = store.create().await.unwrap();
        let keep = store.create().await.unwrap();
        let (id, mut events) = {
            let mut session = target.lock().await;
            session.busy = true;
            (session.record.id.clone(), session.tx.subscribe())
        };
        assert!(store.delete(&id).await.is_err());
        target.lock().await.busy = false;
        std::fs::write(root.path().join("config.json"), b"{}").unwrap();
        assert!(store.delete("CONFIG").await.is_err());
        assert!(root.path().join("config.json").exists());
        let partial = root.path().join(format!("{id}.json.tmp"));
        std::fs::write(&partial, "partial").unwrap();
        store.delete(&id).await.unwrap();
        assert_eq!(events.try_recv().unwrap(), json!({"type":"session_deleted", "session_id":id}));
        assert!(!partial.exists());
        assert!(store.start(target, "旧连接".into()).await.is_err());
        assert!(store.get(&id).await.is_err());
        assert!(store.delete(&id).await.is_err());
        let remaining = Store::new(root.path().into()).list().await.unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0]["id"], keep.lock().await.record.id);
    }

    #[tokio::test]
    async fn snapshots_accumulate_fragments_and_persist_one_final_answer() {
        let root = std::env::temp_dir().join(format!(
            "jisjtu-stream-session-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap()
        ));
        let store = Store::new(root.clone());
        let session = store.create().await.unwrap();
        let mut session = session.lock().await;
        let mut subscriber = session.tx.subscribe();
        session.emit(json!({"type": "answer_delta", "text": "第一段"}));
        session.emit(json!({"type": "answer_delta", "text": "，后续内容"}));
        assert_eq!(subscriber.try_recv().unwrap()["event"]["text"], "第一段");
        assert_eq!(
            subscriber.try_recv().unwrap()["event"]["text"],
            "，后续内容"
        );
        assert_eq!(
            session.snapshot()["events"],
            json!([{"type": "answer_delta", "text": "第一段，后续内容"}])
        );
        session.emit(json!({"type": "answer", "text": "第一段，后续内容"}));
        store.persist(&mut session);
        assert!(session.save_error.is_none());
        let loaded = Store::new(root.clone())
            .get(&session.record.id)
            .await
            .unwrap();
        let loaded = loaded.lock().await;
        assert_eq!(
            loaded.record.events,
            vec![json!({"type": "answer", "text": "第一段，后续内容"})]
        );
        assert_eq!(loaded.record.revision, 3);
        std::fs::remove_dir_all(root).unwrap();
    }

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
