//! export_calendar：把本学期课表、考试和 Canvas 截止事项导出成 iCalendar（.ics）文件，
//! 可以直接导入系统日历、手机日历或 Google 日历。

use std::path::PathBuf;

use chrono::{DateTime, Duration, NaiveDate, NaiveTime, TimeZone, Utc};
use serde_json::Value;

use super::campus_time::{self, PERIODS, date_of, week_of};
use super::canvas::Canvas;
use super::eduinfo::{self, Class, Exam, TermCode};
use super::get_deadlines;

const DEADLINE_DAYS: i64 = 60;

#[derive(Debug, Clone)]
pub(super) struct Event {
    pub uid: String,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub summary: String,
    pub location: Option<String>,
    pub description: Option<String>,
    /// 提前多少分钟提醒
    pub alarms: Vec<i64>,
}

/// FNV-1a：UID 需要在多次导出间保持稳定，重复导入时日历会更新而不是重复添加。
pub(super) fn stable_hash(text: &str) -> u64 {
    text.bytes()
        .fold(0xcbf29ce484222325u64, |hash, byte| (hash ^ byte as u64).wrapping_mul(0x100000001b3))
}

fn beijing_at(date: NaiveDate, time: &str) -> Option<DateTime<Utc>> {
    let time = NaiveTime::parse_from_str(time, "%H:%M").ok()?;
    campus_time::beijing()
        .from_local_datetime(&date.and_time(time))
        .single()
        .map(|time| time.with_timezone(&Utc))
}

/// 课表按周次展开成具体日期的事件；节假日按校历停课，不生成。
pub(super) fn class_events(classes: &[Class], term_start: NaiveDate) -> Vec<Event> {
    let Some(info) = week_of(term_start) else { return Vec::new() };
    let mut events = Vec::new();
    for class in classes {
        let weeks: Vec<u32> = if class.weeks.is_empty() {
            (1..=info.term.weeks - info.term.exam_weeks).collect()
        } else {
            class.weeks.iter().copied().collect()
        };
        for week in weeks.into_iter().filter(|week| *week <= info.term.weeks) {
            let Some(date) = date_of(&info, week, class.weekday) else { continue };
            if info.holiday(date).is_some() {
                continue;
            }
            let period = |number: u32| PERIODS.get(number.checked_sub(1)? as usize);
            let (Some(first), Some(last)) = (period(class.start), period(class.end)) else {
                continue;
            };
            let (Some(start), Some(end)) = (beijing_at(date, first.0), beijing_at(date, last.1)) else { continue };
            let mut description = format!("第 {week} 周 · 第 {}–{} 节", class.start, class.end);
            if !class.teacher.is_empty() {
                description.push_str(&format!(" · {}", class.teacher));
            }
            events.push(Event {
                uid: format!("class-{:016x}@jisjtu", stable_hash(&format!("{}|{date}|{}", class.name, class.start))),
                start,
                end,
                summary: class.name.clone(),
                location: (!class.room.is_empty()).then(|| class.room.clone()),
                description: Some(description),
                alarms: Vec::new(),
            });
        }
    }
    events
}

pub(super) fn exam_events(exams: &[Exam]) -> Vec<Event> {
    exams
        .iter()
        .filter_map(|exam| {
            let (date, start, end) = exam.when.as_ref()?;
            let (start, end) = (beijing_at(*date, start)?, beijing_at(*date, end)?);
            let unknown = |text: &str| text.is_empty() || text.contains("未放开") || text.contains("不可查");
            let location = (!unknown(&exam.room)).then(|| {
                if unknown(&exam.campus) || exam.campus.is_empty() {
                    exam.room.clone()
                } else {
                    format!("{}（{}）", exam.room, exam.campus)
                }
            });
            Some(Event {
                uid: format!("exam-{:016x}@jisjtu", stable_hash(&format!("{}|{}|{date}", exam.course, exam.name))),
                start,
                end,
                summary: format!("【考试】{}", exam.course),
                location,
                description: (!exam.name.is_empty()).then(|| exam.name.clone()),
                // 前一天晚上和开考前一小时各提醒一次
                alarms: vec![24 * 60, 60],
            })
        })
        .collect()
}

/// (截止时间, 课程, 标题, 链接)
pub(super) fn deadline_events(deadlines: &[(DateTime<Utc>, String, String, Option<String>)]) -> Vec<Event> {
    deadlines
        .iter()
        .map(|(due, course, title, url)| Event {
            uid: format!("ddl-{:016x}@jisjtu", stable_hash(&format!("{course}|{title}|{due}"))),
            start: *due - Duration::minutes(30),
            end: *due,
            summary: format!("【DDL】{course} · {title}"),
            location: None,
            description: url.clone(),
            alarms: vec![24 * 60, 3 * 60],
        })
        .collect()
}

/// TEXT 值需要转义反斜杠、分号、逗号和换行。
fn escape(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace(';', "\\;")
        .replace(',', "\\,")
        .replace("\r\n", "\\n")
        .replace('\n', "\\n")
}

/// 每行不超过 75 个字节，续行以一个空格开头；按字符边界折行，避免切断中文。
fn fold(line: &str) -> String {
    let mut out = String::new();
    let mut width = 0;
    for ch in line.chars() {
        let size = ch.len_utf8();
        if width + size > 75 {
            out.push_str("\r\n ");
            width = 1;
        }
        out.push(ch);
        width += size;
    }
    out.push_str("\r\n");
    out
}

fn stamp(time: DateTime<Utc>) -> String {
    time.format("%Y%m%dT%H%M%SZ").to_string()
}

pub(super) fn render_ics(name: &str, events: &[Event], now: DateTime<Utc>) -> String {
    let mut text = String::new();
    for line in [
        "BEGIN:VCALENDAR".to_string(),
        "VERSION:2.0".to_string(),
        "PRODID:-//JIsjtu//Campus Calendar//ZH".to_string(),
        "CALSCALE:GREGORIAN".to_string(),
        "METHOD:PUBLISH".to_string(),
        format!("X-WR-CALNAME:{}", escape(name)),
        "X-WR-TIMEZONE:Asia/Shanghai".to_string(),
    ] {
        text.push_str(&fold(&line));
    }
    for event in events {
        let mut lines = vec![
            "BEGIN:VEVENT".to_string(),
            format!("UID:{}", event.uid),
            format!("DTSTAMP:{}", stamp(now)),
            format!("DTSTART:{}", stamp(event.start)),
            format!("DTEND:{}", stamp(event.end)),
            format!("SUMMARY:{}", escape(&event.summary)),
        ];
        if let Some(location) = &event.location {
            lines.push(format!("LOCATION:{}", escape(location)));
        }
        if let Some(description) = &event.description {
            lines.push(format!("DESCRIPTION:{}", escape(description)));
        }
        for minutes in &event.alarms {
            lines.extend([
                "BEGIN:VALARM".to_string(),
                "ACTION:DISPLAY".to_string(),
                format!("DESCRIPTION:{}", escape(&event.summary)),
                format!("TRIGGER:-PT{minutes}M"),
                "END:VALARM".to_string(),
            ]);
        }
        lines.push("END:VEVENT".to_string());
        for line in lines {
            text.push_str(&fold(&line));
        }
    }
    text.push_str(&fold("END:VCALENDAR"));
    text
}

fn default_path(term: TermCode) -> PathBuf {
    super::downloads::directory()
        .join(format!("交我集-{}-{}.ics", term.xnm, term.season()))
}

fn wants(arguments: &Value, part: &str) -> bool {
    match arguments["include"].as_array() {
        Some(items) if !items.is_empty() => items.iter().any(|item| item.as_str() == Some(part)),
        _ => true,
    }
}

pub(super) async fn export_calendar(arguments: Value) -> Result<String, String> {
    let now = campus_time::now();
    let today = now.date_naive();
    let term = TermCode::current(today);
    let path = arguments["path"]
        .as_str()
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| default_path(term));
    if path.extension().and_then(|ext| ext.to_str()) != Some("ics") {
        return Err("path 需要以 .ics 结尾".into());
    }
    let (want_classes, want_exams, want_deadlines) =
        (wants(&arguments, "classes"), wants(&arguments, "exams"), wants(&arguments, "deadlines"));

    let campus = async {
        if !(want_classes || want_exams) {
            return None;
        }
        let mut requests = Vec::new();
        if want_classes {
            requests.push((eduinfo::schedule_request(term), "课表"));
        }
        if want_exams {
            requests.push((eduinfo::exams_request(term), "考试安排"));
        }
        Some(eduinfo::query(requests).await)
    };
    let planner = async {
        if !want_deadlines {
            return None;
        }
        let canvas = match Canvas::from_env() {
            Ok(canvas) => canvas,
            Err(error) => return Some(Err(error)),
        };
        Some(get_deadlines::fetch_planner(&canvas, now, DEADLINE_DAYS).await)
    };
    let (campus, planner) = tokio::join!(campus, planner);

    let mut events = Vec::new();
    let mut report = Vec::new();
    match campus {
        None => {}
        Some(Err(error)) => report.push(format!("课表和考试获取失败：{error}")),
        Some(Ok(mut results)) => {
            if want_exams {
                match results.pop().expect("考试请求") {
                    Ok(value) => {
                        let exams = eduinfo::exams_from(&value);
                        let found = exam_events(&exams);
                        let pending = exams.len() - found.len();
                        report.push(format!(
                            "考试 {} 场（每场提前一天和一小时提醒）{}",
                            found.len(),
                            if pending > 0 { format!("，另有 {pending} 场时间未公布，公布后重新导出即可") } else { String::new() }
                        ));
                        events.extend(found);
                    }
                    Err(error) => report.push(format!("考试获取失败：{error}")),
                }
            }
            if want_classes {
                match results.pop().expect("课表请求") {
                    Ok(value) => match week_of(today) {
                        Some(info) => {
                            let found = class_events(&eduinfo::classes_from(&value), info.term.start);
                            report.push(format!(
                                "课程 {} 次（{} 第 1–{} 周，已按单双周展开，节假日不生成）",
                                found.len(),
                                info.term.name,
                                info.term.weeks
                            ));
                            events.extend(found);
                        }
                        None => report.push("当前不在已知学期内，无法把课表换算成具体日期".into()),
                    },
                    Err(error) => report.push(format!("课表获取失败：{error}")),
                }
            }
        }
    }
    match planner {
        None => {}
        Some(Err(error)) => report.push(format!("作业截止获取失败：{error}")),
        Some(Ok(items)) => {
            let deadlines = get_deadlines::pending_deadlines(&items, now);
            report.push(format!("作业截止 {} 项（未来 {DEADLINE_DAYS} 天内未完成的，提前一天和三小时提醒）", deadlines.len()));
            events.extend(deadline_events(&deadlines));
        }
    }
    if events.is_empty() {
        return Err(format!("没有可导出的日程。{}", report.join("；")));
    }
    events.sort_by_key(|event| event.start);
    let ics = render_ics(&format!("交我集 · {}", term.label()), &events, Utc::now());
    if let Some(parent) = path.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|err| format!("创建目录失败: {err}"))?;
    }
    std::fs::write(&path, ics).map_err(|err| format!("写入 {} 失败: {err}", path.display()))?;
    Ok(format!(
        "已导出 {} 个日程到 {}\n- {}\n导入方法：macOS 双击文件即可加入“日历”；手机可把文件发给自己后用系统日历打开，或在 Google 日历网页版“导入”。\
         重复导入同一学期的文件会更新已有日程，不会重复。作业截止是导出时的快照，已提交的作业请在日历里手动删除。",
        events.len(),
        path.display(),
        report.join("\n- ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn utc(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text).unwrap().with_timezone(&Utc)
    }

    #[test]
    fn classes_expand_by_week_parity_skip_holidays_and_use_utc() {
        let classes = eduinfo::classes_from(&json!({"kbList": [
            {"kcmc": "概率统计", "xm": "刘老师", "cdmc": "东下院305", "jcs": "1-2", "xqj": "3", "zcd": "1-15周(单)"},
            {"kcmc": "形势与政策", "cdmc": "", "jcs": "11-12", "xqj": "4", "zcd": "3周,11周"}
        ]}));
        let start = NaiveDate::from_ymd_opt(2026, 9, 14).unwrap();
        let events = class_events(&classes, start);
        let stats: Vec<_> = events.iter().filter(|event| event.summary == "概率统计").collect();
        assert_eq!(stats.len(), 8, "单周 1,3,…,15");
        assert_eq!(stats[0].start, utc("2026-09-16T00:00:00Z"), "北京 08:00 = UTC 00:00");
        assert_eq!(stats[0].end, utc("2026-09-16T01:40:00Z"));
        assert_eq!(stats[0].location.as_deref(), Some("东下院305"));
        assert!(stats[0].description.as_deref().unwrap().contains("第 1 周 · 第 1–2 节 · 刘老师"));
        // 第 3 周周四是 10 月 1 日国庆，停课
        let policy: Vec<_> = events.iter().filter(|event| event.summary == "形势与政策").collect();
        assert_eq!(policy.len(), 1);
        assert_eq!(policy[0].start, utc("2026-11-26T10:00:00Z"));
        assert_eq!(class_events(&classes, start)[0].uid, events[0].uid, "UID 在多次导出间稳定");
    }

    #[test]
    fn ics_is_folded_escaped_and_has_alarms() {
        let exams = eduinfo::exams_from(&json!({"items": [
            {"kcmc": "大学物理", "ksmc": "期末考试", "kssj": "2027-01-08(13:00-15:00)", "cdmc": "东上院102", "cdxqmc": "闵行"},
            {"kcmc": "思政", "ksmc": "期末考试", "kssj": "未放开不可查", "cdmc": "未放开不可查"}
        ]}));
        let mut events = exam_events(&exams);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].location.as_deref(), Some("东上院102（闵行）"));
        events.extend(deadline_events(&[(
            utc("2026-11-28T15:59:00Z"),
            "概率统计".into(),
            "第五章作业,含附加题;请看说明".into(),
            Some("https://oc.sjtu.edu.cn/courses/1/assignments/2".into()),
        )]));
        let ics = render_ics("交我集 · 2026-2027 学年秋季学期", &events, utc("2026-11-25T00:00:00Z"));
        assert!(ics.starts_with("BEGIN:VCALENDAR\r\nVERSION:2.0\r\n"));
        assert!(ics.ends_with("END:VCALENDAR\r\n"));
        assert!(ics.contains("DTSTART:20270108T050000Z\r\nDTEND:20270108T070000Z"), "{ics}");
        assert!(ics.contains("TRIGGER:-PT1440M") && ics.contains("TRIGGER:-PT60M"));
        assert!(ics.replace("\r\n ", "").contains("第五章作业\\,含附加题\\;请看说明"), "{ics}");
        for line in ics.split("\r\n") {
            assert!(line.len() <= 75, "超过 75 字节：{line}");
        }
        let unfolded = ics.replace("\r\n ", "");
        assert!(unfolded.contains("SUMMARY:【DDL】概率统计 · 第五章作业\\,含附加题\\;请看说明"), "{unfolded}");
        assert_eq!(ics.matches("BEGIN:VEVENT").count(), 2);
    }

    #[test]
    fn include_filter_defaults_to_everything() {
        assert!(wants(&json!({}), "classes"));
        assert!(wants(&json!({"include": []}), "exams"));
        assert!(wants(&json!({"include": ["exams"]}), "exams"));
        assert!(!wants(&json!({"include": ["exams"]}), "classes"));
    }
}
