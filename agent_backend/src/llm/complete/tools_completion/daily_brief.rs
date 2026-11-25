//! daily_brief：今日简报。并发汇总课表、考试、Canvas 截止事项与公告、未读邮件，
//! 每一块独立获取，某一块失败或未配置不影响其他部分。

use std::future::Future;
use std::time::Duration as StdDuration;

use chrono::{DateTime, Datelike, Duration, FixedOffset, Timelike};
use serde_json::Value;

use super::campus_time::{self, weekday_cn};
use super::canvas::Canvas;
use super::clip;
use super::eduinfo::{self, Class, Exam, TermCode};
use super::{get_announcements, get_deadlines, mail_fetch};

const SOURCE_TIMEOUT: StdDuration = StdDuration::from_secs(90);
const DEADLINE_HOURS: i64 = 48;
const EXAM_DAYS: i64 = 14;

type Part<T> = Option<Result<T, String>>;

async fn within<T>(future: impl Future<Output = Result<T, String>>) -> Result<T, String> {
    tokio::time::timeout(SOURCE_TIMEOUT, future)
        .await
        .unwrap_or_else(|_| Err("超时".into()))
}

fn configured(key: &str) -> bool {
    std::env::var(key).is_ok_and(|value| !value.trim().is_empty())
}

fn greeting(hour: u32) -> &'static str {
    match hour {
        5..=10 => "早上好",
        11..=12 => "中午好",
        13..=17 => "下午好",
        18..=23 => "晚上好",
        _ => "夜深了，记得早点休息",
    }
}

pub(super) struct Brief {
    pub now: DateTime<FixedOffset>,
    pub classes: Part<Vec<Class>>,
    pub exams: Part<Vec<Exam>>,
    pub deadlines: Part<(Vec<String>, Vec<String>)>,
    pub notices: Part<Vec<String>>,
    pub mail: Part<(usize, Vec<String>)>,
}

fn section<T>(text: &mut String, title: &str, part: &Part<T>, missing: &str, body: impl FnOnce(&mut String, &T)) {
    text.push_str(&format!("\n【{title}】"));
    match part {
        None => text.push_str(&format!("{missing}\n")),
        Some(Err(error)) => text.push_str(&format!("获取失败：{error}\n")),
        Some(Ok(value)) => body(text, value),
    }
}

pub(super) fn render(brief: &Brief) -> String {
    let now = brief.now;
    let today = now.date_naive();
    let mut text = format!(
        "{}！今天是 {}月{}日 {}",
        greeting(now.hour()),
        today.month(),
        today.day(),
        weekday_cn(today.weekday())
    );
    match campus_time::week_of(today) {
        Some(info) => {
            text.push_str(&format!("，{}第 {} 周", info.term.name, info.week));
            if info.is_exam_week() {
                text.push_str("（考试周）");
            }
            if let Some(holiday) = info.holiday(today) {
                text.push_str(&format!("，{holiday}放假"));
            }
        }
        None => text.push_str("，当前日期不在已知学期的教学周内"),
    }
    text.push_str(&format!("，现在 {:02}:{:02}。\n", now.hour(), now.minute()));

    section(&mut text, "课程", &brief.classes, "未配置 jAccount，无法读取课表", |text, classes| {
        text.push('\n');
        text.push_str(&eduinfo::render_day(classes, today, now));
        // 晚上顺便看明天
        if now.hour() >= 18 {
            text.push_str(&eduinfo::render_day(classes, today + Duration::days(1), now));
        }
    });

    section(&mut text, "考试", &brief.exams, "未配置 jAccount，无法读取考试安排", |text, exams| {
        let soon: Vec<&Exam> = exams
            .iter()
            .filter(|exam| {
                exam.when
                    .as_ref()
                    .is_some_and(|(date, _, _)| *date >= today && *date <= today + Duration::days(EXAM_DAYS))
            })
            .collect();
        let unknown = exams.iter().filter(|exam| exam.when.is_none()).count();
        if soon.is_empty() {
            text.push_str(&format!("{EXAM_DAYS} 天内没有考试"));
        } else {
            text.push_str(&format!("{EXAM_DAYS} 天内 {} 场：", soon.len()));
        }
        if unknown > 0 {
            text.push_str(&format!("（另有 {unknown} 场时间未公布）"));
        }
        text.push('\n');
        for exam in soon {
            text.push_str(&format!("- {}\n", eduinfo::exam_line(exam, today)));
        }
    });

    section(&mut text, "DDL", &brief.deadlines, "未配置 Canvas 令牌，无法读取作业截止", |text, (soon, overdue)| {
        if soon.is_empty() {
            text.push_str(&format!("{DEADLINE_HOURS} 小时内没有要交的作业\n"));
        } else {
            text.push_str(&format!("{DEADLINE_HOURS} 小时内截止 {} 项：\n", soon.len()));
            for line in soon {
                text.push_str(&format!("- {line}\n"));
            }
        }
        if !overdue.is_empty() {
            text.push_str(&format!("已截止还没交 {} 项（可能还能补交）：\n", overdue.len()));
            for line in overdue.iter().take(5) {
                text.push_str(&format!("- {line}\n"));
            }
        }
    });

    section(&mut text, "公告", &brief.notices, "未配置 Canvas 令牌，无法读取课程公告", |text, notices| {
        if notices.is_empty() {
            text.push_str("最近 24 小时没有新公告\n");
        } else {
            text.push_str(&format!("最近 24 小时 {} 条：\n", notices.len()));
            for line in notices.iter().take(8) {
                text.push_str(&format!("- {line}\n"));
            }
        }
    });

    section(&mut text, "邮件", &brief.mail, "未配置邮箱（IMAP_HOST），跳过", |text, (total, latest)| {
        if *total == 0 {
            text.push_str("收件箱没有未读邮件\n");
        } else {
            text.push_str(&format!("收件箱未读 {total} 封，最新：\n"));
            for line in latest {
                text.push_str(&format!("- {line}\n"));
            }
        }
    });
    text.push_str("\n需要细节时可再查对应工具（课表、考试、DDL、公告、邮件）。");
    text
}

pub(super) async fn daily_brief(_: Value) -> Result<String, String> {
    let now = campus_time::now();
    let term = TermCode::current(now.date_naive());
    let canvas = Canvas::from_env().ok();

    let campus = async {
        if !configured("EMAIL_USER_ACCOUNT") {
            return None;
        }
        // 课表和考试在同一个浏览器会话里查
        let requests = vec![
            (eduinfo::schedule_request(term), "课表"),
            (eduinfo::exams_request(term), "考试安排"),
        ];
        Some(within(eduinfo::query(requests)).await)
    };
    let planner = async {
        let canvas = canvas.as_ref()?;
        Some(
            within(get_deadlines::fetch_planner(canvas, now, 3))
                .await
                .map(|items| get_deadlines::brief_lines(&items, now, DEADLINE_HOURS)),
        )
    };
    let notices = async {
        let canvas = canvas.as_ref()?;
        Some(
            within(get_announcements::fetch_announcements(canvas, None, now, 1))
                .await
                .map(|(mut items, names)| get_announcements::brief_lines(&mut items, &names, now)),
        )
    };
    let mail = async {
        if !configured("IMAP_HOST") {
            return None;
        }
        Some(within(mail_fetch::unread_brief(3)).await)
    };
    let (campus, deadlines, notices, mail) = tokio::join!(campus, planner, notices, mail);

    let (classes, exams) = match campus {
        None => (None, None),
        Some(Err(error)) => (Some(Err(error.clone())), Some(Err(error))),
        Some(Ok(mut results)) => {
            let exams = results.pop().map(|result| result.map(|value| eduinfo::exams_from(&value)));
            let classes = results.pop().map(|result| result.map(|value| eduinfo::classes_from(&value)));
            (classes, exams)
        }
    };
    Ok(clip(&render(&Brief { now, classes, exams, deadlines, notices, mail })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use serde_json::json;

    #[test]
    fn brief_combines_sections_and_isolates_failures() {
        let now = campus_time::beijing().with_ymd_and_hms(2026, 11, 25, 19, 30, 0).unwrap();
        let classes = eduinfo::classes_from(&json!({"kbList": [
            {"kcmc": "大学物理", "cdmc": "东上院102", "jcs": "3-4", "xqj": "3", "zcd": "1-16周"},
            {"kcmc": "形势与政策", "cdmc": "东中院1-101", "jcs": "11-12", "xqj": "4", "zcd": "1-16周"}
        ]}));
        let exams = eduinfo::exams_from(&json!({"items": [
            {"kcmc": "概率统计", "ksmc": "期中考试", "kssj": "2026-12-02(13:00-15:00)", "cdmc": "东上院102", "cdxqmc": "闵行"},
            {"kcmc": "大学物理", "ksmc": "期末考试", "kssj": "未放开不可查", "cdmc": "未放开不可查"}
        ]}));
        let brief = Brief {
            now,
            classes: Some(Ok(classes)),
            exams: Some(Ok(exams)),
            deadlines: Some(Ok((vec!["明天 周四 23:59 · 高数 · 作业 · 习题 5（未提交，还剩 1 天 4 小时）".into()], vec![]))),
            notices: Some(Err("Canvas 认证失败 (401)".into())),
            mail: None,
        };
        let text = render(&brief);
        assert!(text.starts_with("晚上好！今天是 11月25日 周三，2026-2027 学年秋季学期第 11 周，现在 19:30。"), "{text}");
        assert!(text.contains("今天 11月25日 周三（第 11 周）：共 1 节课\n- 第 3–4 节 10:00–11:40 大学物理 @ 东上院102（已结束）"), "{text}");
        assert!(text.contains("明天 11月26日 周四（第 11 周）：共 1 节课\n- 第 11–12 节 18:00–19:40 形势与政策"), "晚上附上明天的课：{text}");
        assert!(text.contains("【考试】14 天内 1 场：（另有 1 场时间未公布）\n- 12月2日 周三 13:00–15:00（还有 7 天）· 概率统计"), "{text}");
        assert!(text.contains("【DDL】48 小时内截止 1 项：\n- 明天 周四 23:59 · 高数"), "{text}");
        assert!(text.contains("【公告】获取失败：Canvas 认证失败 (401)"), "{text}");
        assert!(text.contains("【邮件】未配置邮箱（IMAP_HOST），跳过"), "{text}");
    }
}
