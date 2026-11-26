mod preferences;

use std::collections::{BTreeMap, hash_map::RandomState};
use std::fs::{self, File};
use std::hash::BuildHasher;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use chrono::Utc;
use preferences::{Evidence, consolidate, observe, update};
use serde::{Deserialize, Serialize};

#[derive(Default, Serialize, Deserialize)]
struct Memory {
    generation: u64,
    sources: BTreeMap<String, Vec<Evidence>>,
}

#[derive(Default)]
pub struct Turn {
    pub preferences: Vec<&'static str>,
    evidence: Vec<Evidence>,
    generation: u64,
}

#[derive(Clone)]
pub struct Dreaming {
    root: PathBuf,
    enabled: bool,
}

impl Dreaming {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            enabled: std::env::var("AGENT_DREAMING").as_deref() == Ok("1"),
        }
    }

    fn data_path(&self) -> PathBuf {
        self.root.join(".dreaming").join("preferences.json")
    }

    fn read(&self) -> anyhow::Result<Memory> {
        match fs::read(self.data_path()) {
            Ok(data) => Ok(serde_json::from_slice(&data)?),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(Memory::default()),
            Err(error) => Err(error.into()),
        }
    }

    fn has_source(&self, id: &str) -> bool {
        crate::sessions::valid_id(id) && self.root.join(format!("{id}.json")).is_file()
    }

    fn snapshot(&self, now: i64) -> anyhow::Result<Turn> {
        let memory = self.read()?;
        let sources = memory.sources.iter().filter(|(id, _)| self.has_source(id));
        Ok(Turn {
            preferences: consolidate(sources.map(|(_, items)| items.as_slice()), now),
            generation: memory.generation,
            evidence: Vec::new(),
        })
    }

    // Reads see either the old or new complete snapshot. Serialize read-modify-write
    // across servers too, since two local instances may share a sessions directory.
    async fn change(
        &self,
        edit: impl FnOnce(&Self) -> anyhow::Result<Option<Memory>> + Send + 'static,
    ) -> anyhow::Result<()> {
        let store = self.clone();
        tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
            let path = store.data_path();
            let dir = path.parent().expect("memory directory");
            fs::create_dir_all(dir)?;
            let lock = File::options()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(dir.join("write.lock"))?;
            lock.lock()?;
            if let Some(memory) = edit(&store)? {
                save(dir, &path, &memory)?;
            }
            Ok(())
        })
        .await?
    }

    pub async fn begin(&self, prompt: &str, at: i64) -> anyhow::Result<Turn> {
        if !self.enabled {
            return Ok(Turn::default());
        }
        let store = self.clone();
        let mut turn = tokio::task::spawn_blocking(move || store.snapshot(at)).await??;
        turn.evidence = observe(prompt, at);
        Ok(turn)
    }

    pub async fn finish(&self, source: String, turn: Turn) -> anyhow::Result<()> {
        if turn.evidence.is_empty() {
            return Ok(());
        }
        self.change(move |store| {
            let mut memory = store.read()?;
            // A forget command may arrive while a model call is still running.
            if memory.generation != turn.generation || !store.has_source(&source) {
                return Ok(None);
            }
            let now = Utc::now().timestamp_millis();
            memory.sources.retain(|id, items| {
                items.retain(|item| item.is_fresh(now));
                !items.is_empty() && store.has_source(id)
            });
            update(memory.sources.entry(source).or_default(), turn.evidence);
            Ok(Some(memory))
        })
        .await
    }

    pub async fn remove_source(&self, id: &str) -> anyhow::Result<()> {
        if !self.data_path().try_exists()? {
            return Ok(());
        }
        let id = id.to_owned();
        self.change(move |store| {
            let mut memory = store.read()?;
            Ok(memory.sources.remove(&id).map(|_| memory))
        })
        .await
    }

    pub async fn status(&self) -> anyhow::Result<String> {
        if !self.enabled {
            return Ok(
                "跨会话偏好记忆已关闭。在 .env 中设置 AGENT_DREAMING=1 并重启即可启用。".into(),
            );
        }
        let store = self.clone();
        let turn =
            tokio::task::spawn_blocking(move || store.snapshot(Utc::now().timestamp_millis()))
                .await??;
        if turn.preferences.is_empty() {
            return Ok("跨会话偏好记忆已开启，目前没有已形成的偏好。".into());
        }
        Ok(format!(
            "当前跨会话答复偏好\n- {}",
            turn.preferences.join("\n- ")
        ))
    }

    pub async fn forget(&self) -> anyhow::Result<String> {
        // A fresh nonzero generation invalidates earlier turns even if the old
        // file was lost or damaged, or an enabled peer has not saved its first turn.
        self.change(|_| {
            Ok(Some(Memory {
                generation: RandomState::new()
                    .hash_one(Utc::now().timestamp_nanos_opt())
                    .max(1),
                sources: BTreeMap::new(),
            }))
        })
        .await?;
        Ok("已清空跨会话答复偏好及尚未形成偏好的记录。".into())
    }
}

fn save(dir: &Path, path: &Path, memory: &Memory) -> anyhow::Result<()> {
    let mut file = tempfile::NamedTempFile::new_in(dir)?;
    serde_json::to_writer(file.as_file_mut(), memory)?;
    file.as_file().sync_all()?;
    file.persist(path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile() -> anyhow::Result<(tempfile::TempDir, Dreaming)> {
        let root = tempfile::tempdir()?;
        let store = Dreaming {
            root: root.path().to_owned(),
            enabled: true,
        };
        Ok((root, store))
    }

    fn source(store: &Dreaming, id: &str) -> anyhow::Result<()> {
        Ok(fs::write(store.root.join(format!("{id}.json")), "{}")?)
    }

    #[tokio::test]
    async fn independent_stores_merge_concurrent_finishes() -> anyhow::Result<()> {
        let (_root, first) = profile()?;
        let second = Dreaming {
            root: first.root.clone(),
            enabled: true,
        };
        source(&first, "a")?;
        source(&second, "b")?;
        let at = Utc::now().timestamp_millis();
        let a = first.begin("请分点回答。", at).await?;
        let b = second.begin("请分点回答。", at).await?;
        tokio::try_join!(first.finish("a".into(), a), second.finish("b".into(), b))?;
        assert_eq!(first.read()?.sources.len(), 2);
        assert_eq!(second.snapshot(at)?.preferences, ["默认分点回答。"]);
        Ok(())
    }

    #[tokio::test]
    async fn forget_recovers_damaged_state_and_rejects_earlier_turns() -> anyhow::Result<()> {
        let (_root, active) = profile()?;
        source(&active, "a")?;
        let other = Dreaming {
            root: active.root.clone(),
            enabled: false,
        };
        let at = Utc::now().timestamp_millis();
        // Other servers do not know whether an enabled peer has read an empty store.
        let first = active.begin("默认用英文回答。", at).await?;
        other.forget().await?;
        active.finish("a".into(), first).await?;
        assert!(active.read()?.sources.is_empty());

        let ongoing = active.begin("以后用中文回答。", at).await?;
        let previous = ongoing.generation;
        fs::write(active.data_path(), b"{broken")?;
        other.forget().await?;
        assert_ne!(previous, active.read()?.generation);
        active.finish("a".into(), ongoing).await?;
        assert!(active.snapshot(at)?.preferences.is_empty());

        let turn = active.begin("以后用中文回答。", at).await?;
        tokio::try_join!(active.finish("a".into(), turn), other.forget())?;
        assert!(active.read()?.sources.is_empty());
        let after_reset = active.begin("默认用英文回答。", at).await?;
        active.finish("a".into(), after_reset).await?;
        assert_eq!(active.snapshot(at)?.preferences, ["默认用英文回答。"]);
        Ok(())
    }

    #[tokio::test]
    async fn deleted_and_missing_sources_cannot_support_recall() -> anyhow::Result<()> {
        let (_root, store) = profile()?;
        let at = Utc::now().timestamp_millis();
        for id in ["a", "b"] {
            source(&store, id)?;
            let turn = store.begin("请分点回答。", at).await?;
            store.finish(id.into(), turn).await?;
        }
        assert_eq!(store.snapshot(at)?.preferences, ["默认分点回答。"]);
        let ongoing = store.begin("默认用中文回答。", at).await?;
        fs::remove_file(store.root.join("a.json"))?;
        assert!(store.snapshot(at)?.preferences.is_empty());
        store.remove_source("a").await?;
        store.finish("a".into(), ongoing).await?;
        assert!(!store.read()?.sources.contains_key("a"));
        store.remove_source("b").await?;
        assert!(store.read()?.sources.is_empty());
        Ok(())
    }
}
