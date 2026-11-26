//! One user-requested Canvas attendance watch per course and class start.
use std::collections::HashSet;
use std::fs::{self, File, TryLockError};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use chrono::{DateTime, TimeDelta, Utc};
use serde::{Deserialize, Serialize};
use tokio::time::{sleep, timeout};

use super::attendance_source::AttendanceSource;
use super::campus_time::beijing;
use super::export_calendar::stable_hash;
use super::reminders::notify_once;

const BEFORE: TimeDelta = TimeDelta::minutes(10);
const AFTER: TimeDelta = TimeDelta::minutes(5);
const PERIOD: TimeDelta = TimeDelta::seconds(30);
const READ_TIMEOUT: Duration = Duration::from_secs(29);

#[derive(Serialize, Deserialize)]
struct Watch {
    course_id: u64,
    course_name: String,
    start: i64,
}

struct Scheduler {
    directory: PathBuf,
    active: Mutex<HashSet<PathBuf>>,
}

static SCHEDULER: OnceLock<Scheduler> = OnceLock::new();

impl Watch {
    fn filename(&self) -> String {
        format!("{}-{}.json", self.start, self.course_id)
    }
}

pub(crate) fn is_running() -> bool {
    SCHEDULER.get().is_some()
}

pub(crate) fn start() {
    let scheduler = Scheduler {
        directory: crate::sessions::dir().join(".reminders").join("attendance"),
        active: Mutex::default(),
    };
    if SCHEDULER.set(scheduler).is_err() {
        return;
    }
    let scheduler = SCHEDULER.get().unwrap();
    let entries = match scheduler.directory.read_dir() {
        Ok(entries) => entries,
        Err(error) if error.kind() == ErrorKind::NotFound => return,
        Err(error) => {
            eprintln!("无法恢复签到监看任务：{error}");
            return;
        }
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) == Some("json") {
            scheduler.launch(path);
        } else if path.extension().and_then(|s| s.to_str()) == Some("receipt") {
            let expiry = entry
                .file_name()
                .to_str()
                .and_then(|name| name.split_once('-')?.0.parse::<i64>().ok());
            if expiry.is_some_and(|expires| expires < Utc::now().timestamp()) {
                let _ = fs::remove_file(path);
            }
        }
    }
}

pub(super) fn schedule(
    course_id: u64,
    course_name: &str,
    start: DateTime<Utc>,
) -> Result<String, String> {
    let scheduler = SCHEDULER
        .get()
        .ok_or("请在运行中的 JIsjtu 对话中安排签到监看")?;
    let open = start - BEFORE;
    let end = start + AFTER;
    if Utc::now() > end {
        return Err("这节课的签到监看时段已经结束".into());
    }
    let watch = Watch {
        course_id,
        course_name: course_name.to_owned(),
        start: start.timestamp(),
    };
    fs::create_dir_all(&scheduler.directory).map_err(|e| format!("无法创建签到监看目录：{e}"))?;
    let path = scheduler.directory.join(watch.filename());
    let mut staged =
        tempfile::NamedTempFile::new_in(&scheduler.directory).map_err(|e| e.to_string())?;
    serde_json::to_writer(&mut staged, &watch).map_err(|e| e.to_string())?;
    let state = match staged.persist_noclobber(&path) {
        Ok(_) => "已安排",
        Err(error) if error.error.kind() == ErrorKind::AlreadyExists => "已安排过",
        Err(error) => return Err(format!("无法保存签到监看任务：{error}")),
    };
    scheduler.launch(path);
    let local = |time: DateTime<Utc>| {
        time.with_timezone(&beijing())
            .format("%m月%d日 %H:%M")
            .to_string()
    };
    Ok(format!(
        "{state}《{course_name}》的签到监看。北京时间 {} 至 {}，每 30 秒检查一次。发现本节课的新签到活动时发送 macOS 通知。请在监看时段保持 JIsjtu 运行。",
        local(open),
        local(end)
    ))
}

impl Scheduler {
    fn launch(&'static self, path: PathBuf) {
        if !self.active.lock().unwrap().insert(path.clone()) {
            return;
        }
        tokio::spawn(async move {
            if let Err(error) = self.run(&path).await {
                eprintln!("签到监看任务无法执行：{error}");
            }
            self.active.lock().unwrap().remove(&path);
        });
    }

    async fn run(&self, path: &Path) -> Result<(), String> {
        let file = match File::open(path) {
            Ok(file) => file,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e.to_string()),
        };
        let watch: Watch = serde_json::from_reader(&file).map_err(|e| e.to_string())?;
        if path.file_name().and_then(|name| name.to_str()) != Some(watch.filename().as_str()) {
            return Err("签到监看文件与课程不匹配".into());
        }
        let start = DateTime::from_timestamp(watch.start, 0).ok_or("无效的上课时间")?;
        let open = start - BEFORE;
        let end = start + AFTER;
        wait_until(open).await;
        // The OS releases this lock if a process dies; another open instance can take over.
        loop {
            match file.try_lock() {
                Ok(()) => break,
                Err(TryLockError::WouldBlock) if Utc::now() < end => {
                    sleep(Duration::from_secs(5)).await;
                }
                Err(TryLockError::WouldBlock) => return Ok(()),
                Err(TryLockError::Error(e)) => return Err(e.to_string()),
            }
        }
        if path.exists() && Utc::now() <= end {
            self.poll(&watch, open, end).await;
        }
        match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }

    async fn poll(&self, watch: &Watch, open: DateTime<Utc>, end: DateTime<Utc>) {
        let mut tick = Utc::now();
        let mut sent = HashSet::new();
        let mut had_response = false;
        let mut last_error = None;
        let mut source = AttendanceSource::new(watch.course_id);
        let receipt = |key: &str| {
            self.directory.join(format!(
                "{}-{}-{:016x}.receipt",
                end.timestamp(),
                watch.course_id,
                stable_hash(key)
            ))
        };
        loop {
            wait_until(tick).await;
            // Allow ordinary wakeup jitter at the final, inclusive +5:00 tick.
            // After a long system sleep do not make a request beyond the window.
            let requested_at = Utc::now();
            if requested_at > end + TimeDelta::seconds(1) {
                break;
            }
            let budget = if requested_at >= end || tick == end {
                tick = end;
                READ_TIMEOUT
            } else {
                READ_TIMEOUT.min((end - requested_at).to_std().unwrap_or_default())
            };
            match timeout(budget, source.records()).await {
                Ok(Ok(records)) => {
                    had_response = true;
                    let through = Utc::now().min(end);
                    for event in records {
                        if !(open..=through).contains(&event.started_at) || sent.contains(&event.id)
                        {
                            continue;
                        }
                        let time = event.started_at.with_timezone(&beijing()).format("%H:%M");
                        let mut body = format!("《{}》于 {time} 发起签到。", watch.course_name);
                        if let Some(status) = event.status.as_deref().filter(|s| !s.is_empty()) {
                            body.push_str(&format!("个人状态：{status}"));
                        }
                        if notify_once(&receipt(&format!("event:{}", event.id)), "课堂签到", &body)
                            .await
                        {
                            sent.insert(event.id);
                        }
                    }
                }
                Ok(Err(error)) => last_error = Some(error),
                Err(_) => last_error = Some("查询签到记录超时".into()),
            }
            let Some(next) = next_tick(tick, Utc::now(), end) else {
                break;
            };
            tick = next;
        }
        if let Some(error) = last_error.filter(|_| !had_response) {
            let reason: String = error
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .chars()
                .take(100)
                .collect();
            let body = format!(
                "《{}》在监看时段内未能读取签到记录。{reason}",
                watch.course_name
            );
            notify_once(&receipt("failure"), "签到监看失败", &body).await;
        }
    }
}

fn next_tick(
    previous: DateTime<Utc>,
    now: DateTime<Utc>,
    end: DateTime<Utc>,
) -> Option<DateTime<Utc>> {
    if previous >= end || now > end + TimeDelta::seconds(1) {
        return None;
    }
    // Skip missed slots without overlapping requests. Always schedule one final check at +5:00.
    let elapsed = (now - previous).num_milliseconds().max(0);
    let slots = elapsed / PERIOD.num_milliseconds() + 1;
    Some((previous + TimeDelta::seconds(slots * PERIOD.num_seconds())).min(end))
}

async fn wait_until(when: DateTime<Utc>) {
    // Recheck wall time after sleep/resume and after clock adjustments.
    while let Ok(wait) = (when - Utc::now()).to_std() {
        sleep(wait.min(Duration::from_secs(30))).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn covers_both_class_window_boundaries_and_skips_overdue_polls() {
        let class = DateTime::from_timestamp(1_800_000_000, 0).unwrap();
        let begin = class - BEFORE;
        let end = class + AFTER;
        let mut checks = vec![begin];
        while let Some(next) = next_tick(*checks.last().unwrap(), *checks.last().unwrap(), end) {
            checks.push(next);
        }
        assert_eq!(checks.len(), 31);
        assert_eq!(checks.last(), Some(&end));
        assert_eq!(
            next_tick(begin, begin + TimeDelta::seconds(33), end),
            Some(begin + PERIOD * 2)
        );
        assert_eq!(
            next_tick(
                end - TimeDelta::seconds(11),
                end - TimeDelta::seconds(6),
                end
            ),
            Some(end)
        );
        assert_eq!(next_tick(begin, end + TimeDelta::minutes(1), end), None);
    }
}
