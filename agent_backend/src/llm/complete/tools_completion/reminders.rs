use std::collections::HashMap;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, Utc};
use tokio::time::{self, MissedTickBehavior};

use super::campus_time;
use super::canvas::Canvas;
use super::eduinfo::{self, TermCode};
use super::export_calendar::{Event, class_events, deadline_events};
use super::get_deadlines;

enum Source {
    Classes,
    Deadlines(Canvas),
}

impl Source {
    async fn events(&self) -> Result<Vec<Event>, String> {
        let now = campus_time::now();
        match self {
            Self::Classes => {
                let today = now.date_naive();
                let Some(week) = campus_time::week_of(today) else {
                    return Ok(vec![]);
                };
                let classes = eduinfo::cached_classes(TermCode::current(today)).await?;
                Ok(class_events(&classes, week.term.start))
            }
            Self::Deadlines(canvas) => {
                let planner = get_deadlines::fetch_planner(canvas, now, 2).await?;
                Ok(deadline_events(&get_deadlines::pending_deadlines(
                    &planner, now,
                )))
            }
        }
    }

    fn alarm(&self, event: &Event) -> (DateTime<Utc>, i64, &'static str) {
        match self {
            Self::Classes => (event.start, 10, "上课提醒"),
            Self::Deadlines(_) => (event.end, 60, "作业截止提醒"),
        }
    }

    async fn send_due(
        &self,
        events: &[Event],
        sent: &mut HashMap<String, DateTime<Utc>>,
        directory: &Path,
    ) {
        let now = Utc::now();
        sent.retain(|_, expires| *expires > now);
        for event in events {
            let (when, minutes, title) = self.alarm(event);
            if now >= when
                || now < when - chrono::Duration::minutes(minutes)
                || sent.get(&event.uid) == Some(&when)
            {
                continue;
            }
            let clock = when.with_timezone(&campus_time::beijing()).format("%H:%M");
            let mut body = format!("{clock}  {}", event.summary);
            if let Some(room) = &event.location {
                body.push_str(&format!(" · {room}"));
            }
            let receipt = directory.join(format!("{}-{}", when.timestamp(), event.uid));
            if notify_once(&receipt, title, &body).await {
                sent.insert(event.uid.clone(), when);
            }
        }
    }

    async fn monitor(self, directory: PathBuf) {
        let refresh_seconds = match self {
            Self::Classes => 3 * 3600,
            Self::Deadlines(_) => 5 * 60,
        };
        let mut events: Vec<Event> = vec![];
        let mut sent = HashMap::new();
        let mut tick = time::interval(Duration::from_secs(15));
        tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let mut delay = Duration::ZERO;
        loop {
            let sleep = time::sleep(delay);
            let refresh = async {
                sleep.await;
                self.events().await
            };
            tokio::pin!(refresh);
            let result = loop {
                tokio::select! {
                    result = &mut refresh => break result,
                    _ = tick.tick() => self.send_due(&events, &mut sent, &directory).await,
                }
            };
            delay = match result {
                Ok(updated) => {
                    events = updated;
                    Duration::from_secs(refresh_seconds)
                }
                Err(_) => Duration::from_secs(5 * 60),
            };
        }
    }
}

#[cfg(target_os = "macos")]
async fn deliver_notification(title: &str, body: &str) -> bool {
    const SCRIPT: &str =
        "on run argv\n display notification (item 2 of argv) with title (item 1 of argv)\nend run";
    let result = time::timeout(
        Duration::from_secs(8),
        tokio::process::Command::new("/usr/bin/osascript")
            .args(["-e", SCRIPT, "--", title, body])
            .kill_on_drop(true)
            .output(),
    )
    .await;
    matches!(result, Ok(Ok(output)) if output.status.success())
}

#[cfg(windows)]
fn show_toast(title: &str, body: &str) -> windows::core::Result<()> {
    use windows::UI::Notifications::{
        ToastNotification, ToastNotificationManager, ToastTemplateType,
    };
    use windows::core::HSTRING;

    let xml = ToastNotificationManager::GetTemplateContent(ToastTemplateType::ToastText02)?;
    let lines = xml.GetElementsByTagName(&HSTRING::from("text"))?;
    for (index, content) in [(0, title), (1, body)] {
        lines
            .Item(index)?
            .AppendChild(&xml.CreateTextNode(&HSTRING::from(content))?)?;
    }
    let toast = ToastNotification::CreateToastNotification(&xml)?;
    // The installer sets this AppUserModelID on the Start menu shortcut.
    let notifier =
        ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from("JIsjtu.Desktop"))?;
    notifier.Show(&toast)
}

#[cfg(windows)]
async fn deliver_notification(title: &str, body: &str) -> bool {
    let (title, body) = (title.to_owned(), body.to_owned());
    match tokio::task::spawn_blocking(move || show_toast(&title, &body)).await {
        Ok(Ok(())) => true,
        Ok(Err(error)) => {
            eprintln!("Windows 系统通知发送失败：{error}");
            false
        }
        Err(_) => false,
    }
}

pub(super) async fn notify_once(receipt: &Path, title: &str, body: &str) -> bool {
    // Reserve before delivery so two instances using the same profile cannot both notify.
    let reserved = match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(receipt)
    {
        Ok(_) => true,
        Err(err) if err.kind() == ErrorKind::AlreadyExists => return true,
        Err(_) => false, // A read-only profile still gets in-memory deduplication.
    };
    let delivered = deliver_notification(title, body).await;
    if reserved && !delivered {
        let _ = fs::remove_file(receipt);
    }
    delivered
}

pub(crate) fn start() {
    if std::env::var("AGENT_REMINDERS").as_deref() == Ok("0") {
        return;
    }
    let directory = crate::sessions::dir().join(".reminders");
    let _ = fs::create_dir_all(&directory);
    // Only timestamps and event hashes are stored. Drop expired receipts on startup.
    if let Ok(entries) = directory.read_dir() {
        let now = Utc::now().timestamp();
        for entry in entries.flatten() {
            let name = entry.file_name();
            let expiry = name
                .to_str()
                .and_then(|s| s.split_once('-'))
                .and_then(|(t, _)| t.parse::<i64>().ok());
            if expiry.is_some_and(|t| t < now) {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
    if std::env::var("EMAIL_USER_ACCOUNT").is_ok_and(|user| !user.trim().is_empty()) {
        tokio::spawn(Source::Classes.monitor(directory.clone()));
    }
    if let Ok(canvas) = Canvas::from_env() {
        tokio::spawn(Source::Deadlines(canvas).monitor(directory));
    }
}
