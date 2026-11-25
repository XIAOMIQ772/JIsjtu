//! 北京时间、交大校历（教学周）与作息时间（节次），供各校园工具共用。
//!
//! Canvas 等接口返回 UTC 时间，直接给模型看容易把 23:59 的截止时间读成 15:59，
//! 所以所有时间都在这里统一换成北京时间再输出。

use chrono::{DateTime, Datelike, FixedOffset, NaiveDate, NaiveTime, Timelike, Utc, Weekday};

pub(super) fn beijing() -> FixedOffset {
    FixedOffset::east_opt(8 * 3600).expect("UTC+8 是合法时区")
}

pub(super) fn now() -> DateTime<FixedOffset> {
    Utc::now().with_timezone(&beijing())
}

pub(super) fn weekday_cn(day: Weekday) -> &'static str {
    ["周一", "周二", "周三", "周四", "周五", "周六", "周日"][day.num_days_from_monday() as usize]
}

/// 解析接口返回的 RFC 3339 时间，统一换成北京时间。
pub(super) fn parse_time(text: &str) -> Option<DateTime<FixedOffset>> {
    DateTime::parse_from_rfc3339(text.trim())
        .ok()
        .map(|time| time.with_timezone(&beijing()))
}

/// 形如 “11月28日 周六 23:59”，不在今年时带上年份。
pub(super) fn fmt_time(time: DateTime<FixedOffset>, now: DateTime<FixedOffset>) -> String {
    let date = if time.year() == now.year() {
        format!("{}月{}日", time.month(), time.day())
    } else {
        format!("{}年{}月{}日", time.year(), time.month(), time.day())
    };
    format!(
        "{date} {} {:02}:{:02}",
        weekday_cn(time.weekday()),
        time.hour(),
        time.minute()
    )
}

/// 近三天用“今天/明天/后天”，其余同 fmt_time：“明天 周四 23:59”。
pub(super) fn fmt_day_time(time: DateTime<FixedOffset>, now: DateTime<FixedOffset>) -> String {
    let label = match (time.date_naive() - now.date_naive()).num_days() {
        0 => "今天",
        1 => "明天",
        2 => "后天",
        -1 => "昨天",
        _ => return fmt_time(time, now),
    };
    format!(
        "{label} {} {:02}:{:02}",
        weekday_cn(time.weekday()),
        time.hour(),
        time.minute()
    )
}

/// 截止前显示“还剩…”，截止后显示“已过…”。
pub(super) fn fmt_remaining(due: DateTime<FixedOffset>, now: DateTime<FixedOffset>) -> String {
    let minutes = (due - now).num_minutes();
    let span = minutes.abs();
    let text = if span >= 1440 {
        format!("{} 天 {} 小时", span / 1440, span % 1440 / 60)
    } else if span >= 60 {
        format!("{} 小时 {} 分钟", span / 60, span % 60)
    } else {
        format!("{} 分钟", span.max(1))
    };
    if minutes >= 0 {
        format!("还剩 {text}")
    } else {
        format!("已过 {text}")
    }
}

/// 接口时间字符串 → “11月28日 周六 23:59（还剩 2 天 3 小时）”；解析失败时原样返回。
pub(super) fn describe_deadline(text: &str, now: DateTime<FixedOffset>) -> String {
    match parse_time(text) {
        Some(time) => format!("{}（{}）", fmt_time(time, now), fmt_remaining(time, now)),
        None => text.to_string(),
    }
}

/// 一个学期：第 1 周周一开始，共 `weeks` 周，最后 `exam_weeks` 周为考试周。
#[derive(Clone, Debug)]
pub(super) struct Term {
    pub name: String,
    pub start: NaiveDate,
    pub weeks: u32,
    pub exam_weeks: u32,
    pub holidays: Vec<(NaiveDate, &'static str)>,
}

impl Term {
    #[cfg(test)]
    pub fn end(&self) -> NaiveDate {
        self.start + chrono::Duration::days(self.weeks as i64 * 7 - 1)
    }
}

struct KnownTerm {
    name: &'static str,
    start: (i32, u32, u32),
    weeks: u32,
    exam_weeks: u32,
    holidays: &'static [((i32, u32, u32), &'static str)],
}

/// 教务处公布的校历（jwc.sjtu.edu.cn → 教学校历）。新学期公布后在这里补一行，
/// 或在 .env 设置 SJTU_TERM_START 临时覆盖。
const KNOWN_TERMS: &[KnownTerm] = &[KnownTerm {
    name: "2026-2027 学年秋季学期",
    start: (2026, 9, 14),
    weeks: 18,
    exam_weeks: 2,
    holidays: &[
        ((2026, 9, 25), "中秋节"),
        ((2026, 10, 1), "国庆节"),
        ((2026, 10, 2), "国庆节"),
        ((2026, 10, 3), "国庆节"),
        ((2027, 1, 1), "元旦"),
    ],
}];

fn date(parts: (i32, u32, u32)) -> NaiveDate {
    NaiveDate::from_ymd_opt(parts.0, parts.1, parts.2).expect("校历日期合法")
}

/// .env 中 SJTU_TERM_START（第 1 周周一，YYYY-MM-DD）优先，其次是内置校历。
pub(super) fn terms() -> Vec<Term> {
    let mut terms = Vec::new();
    if let Some(start) = std::env::var("SJTU_TERM_START")
        .ok()
        .and_then(|text| NaiveDate::parse_from_str(text.trim(), "%Y-%m-%d").ok())
    {
        let weeks = std::env::var("SJTU_TERM_WEEKS")
            .ok()
            .and_then(|text| text.trim().parse().ok())
            .filter(|weeks| (1..=30).contains(weeks))
            .unwrap_or(18);
        terms.push(Term {
            name: std::env::var("SJTU_TERM_NAME")
                .ok()
                .filter(|name| !name.trim().is_empty())
                .unwrap_or_else(|| "本学期".into()),
            // 填错成非周一时按所在周的周一计算
            start: start - chrono::Duration::days(start.weekday().num_days_from_monday() as i64),
            weeks,
            exam_weeks: if weeks >= 18 { 2 } else { 0 },
            holidays: Vec::new(),
        });
    }
    terms.extend(KNOWN_TERMS.iter().map(|known| Term {
        name: known.name.to_string(),
        start: date(known.start),
        weeks: known.weeks,
        exam_weeks: known.exam_weeks,
        holidays: known
            .holidays
            .iter()
            .map(|(day, name)| (date(*day), *name))
            .collect(),
    }));
    terms
}

#[derive(Clone, Debug)]
pub(super) struct WeekInfo {
    pub term: Term,
    pub week: u32,
}

impl WeekInfo {
    pub fn is_exam_week(&self) -> bool {
        self.week + self.term.exam_weeks > self.term.weeks
    }

    pub fn holiday(&self, day: NaiveDate) -> Option<&'static str> {
        self.term
            .holidays
            .iter()
            .find(|(date, _)| *date == day)
            .map(|(_, name)| *name)
    }

    /// “2026-2027 学年秋季学期 第 11 周（共 18 周，第 17–18 周为考试周）”
    pub fn describe(&self) -> String {
        let mut text = format!("{} 第 {} 周（共 {} 周", self.term.name, self.week, self.term.weeks);
        if self.term.exam_weeks > 0 {
            let first = self.term.weeks - self.term.exam_weeks + 1;
            if first == self.term.weeks {
                text.push_str(&format!("，第 {first} 周为考试周"));
            } else {
                text.push_str(&format!("，第 {first}–{} 周为考试周", self.term.weeks));
            }
        }
        text.push('）');
        if self.is_exam_week() {
            text.push_str("，本周为考试周");
        }
        text
    }
}

pub(super) fn week_in(terms: &[Term], day: NaiveDate) -> Option<WeekInfo> {
    terms.iter().find_map(|term| {
        let days = (day - term.start).num_days();
        let week = u32::try_from(days / 7 + 1).ok()?;
        (days >= 0 && week <= term.weeks).then(|| WeekInfo {
            term: term.clone(),
            week,
        })
    })
}

pub(super) fn week_of(day: NaiveDate) -> Option<WeekInfo> {
    week_in(&terms(), day)
}

/// 假期里用来提示下学期何时开始。
pub(super) fn next_term(day: NaiveDate) -> Option<Term> {
    terms()
        .into_iter()
        .filter(|term| term.start > day)
        .min_by_key(|term| term.start)
}

/// 某教学周第 `weekday` 天（1=周一）的日期。
pub(super) fn date_of(info: &WeekInfo, week: u32, weekday: u32) -> Option<NaiveDate> {
    let offset = (week.checked_sub(1)? * 7 + weekday.checked_sub(1)?) as i64;
    Some(info.term.start + chrono::Duration::days(offset))
}

/// 教学信息服务网（正方系统）查询用的学年 xnm 与学期 xqm：秋季 3、春季 12、夏季 16。
pub(super) fn academic_term(day: NaiveDate) -> (i32, &'static str, &'static str) {
    match day.month() {
        8..=12 => (day.year(), "3", "秋季学期"),
        1 => (day.year() - 1, "3", "秋季学期"),
        2..=6 => (day.year() - 1, "12", "春季学期"),
        _ => (day.year() - 1, "16", "夏季学期"),
    }
}

/// 交大作息时间（第 1–14 节），节次与教务系统、教室系统的编号一致。
pub(super) const PERIODS: [(&str, &str); 14] = [
    ("08:00", "08:45"),
    ("08:55", "09:40"),
    ("10:00", "10:45"),
    ("10:55", "11:40"),
    ("12:00", "12:45"),
    ("12:55", "13:40"),
    ("14:00", "14:45"),
    ("14:55", "15:40"),
    ("16:00", "16:45"),
    ("16:55", "17:40"),
    ("18:00", "18:45"),
    ("18:55", "19:40"),
    ("20:00", "20:45"),
    ("20:55", "21:40"),
];

fn clock(text: &str) -> NaiveTime {
    NaiveTime::parse_from_str(text, "%H:%M").expect("作息时间格式正确")
}

/// 第 start–end 节的起止时间，如 “08:00–09:40”。
pub(super) fn period_span(start: u32, end: u32) -> Option<String> {
    let first = PERIODS.get(start.checked_sub(1)? as usize)?;
    let last = PERIODS.get(end.checked_sub(1)? as usize)?;
    (start <= end).then(|| format!("{}–{}", first.0, last.1))
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum PeriodNow {
    /// 第 n 节课进行中
    InClass(u32),
    /// 课间或上课前，下一节是第 n 节
    Before(u32),
    /// 今天的课都已结束
    AfterAll,
}

pub(super) fn period_now(time: NaiveTime) -> PeriodNow {
    for (index, (start, end)) in PERIODS.iter().enumerate() {
        let number = index as u32 + 1;
        if time < clock(start) {
            return PeriodNow::Before(number);
        }
        if time < clock(end) {
            return PeriodNow::InClass(number);
        }
    }
    PeriodNow::AfterAll
}

/// get_time_stamp：北京时间、星期、教学周、当前节次。
pub(super) fn describe_now(now: DateTime<FixedOffset>) -> String {
    let today = now.date_naive();
    let mut text = format!(
        "北京时间 {} {}\n",
        now.format("%Y-%m-%d %H:%M:%S"),
        weekday_cn(now.weekday())
    );
    match week_of(today) {
        Some(info) => {
            text.push_str(&info.describe());
            text.push('\n');
            if let Some(holiday) = info.holiday(today) {
                text.push_str(&format!("今天是{holiday}，放假\n"));
            }
        }
        None => {
            text.push_str("当前不在已知学期的教学周内（假期，或校历尚未录入）");
            if let Some(term) = next_term(today) {
                text.push_str(&format!(
                    "；{} 将于 {} 开始",
                    term.name,
                    term.start.format("%Y-%m-%d")
                ));
            }
            text.push('\n');
        }
    }
    match period_now(now.time()) {
        PeriodNow::InClass(n) => {
            let (start, end) = PERIODS[n as usize - 1];
            text.push_str(&format!("当前处于第 {n} 节（{start}–{end}）"));
        }
        PeriodNow::Before(1) => text.push_str("今天的第 1 节课 08:00 开始"),
        PeriodNow::Before(n) => {
            text.push_str(&format!("当前为课间，下一节是第 {n} 节（{} 开始）", PERIODS[n as usize - 1].0))
        }
        PeriodNow::AfterAll => text.push_str("今天的课程时段已全部结束（第 14 节 21:40 下课）"),
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(y: i32, m: u32, d: u32, h: u32, min: u32) -> DateTime<FixedOffset> {
        beijing().with_ymd_and_hms(y, m, d, h, min, 0).unwrap()
    }

    #[test]
    fn teaching_weeks_follow_the_published_calendar() {
        let terms = terms();
        let day = |m, d| NaiveDate::from_ymd_opt(if m >= 9 { 2026 } else { 2027 }, m, d).unwrap();
        assert!(week_in(&terms, day(9, 13)).is_none());
        assert_eq!(week_in(&terms, day(9, 14)).unwrap().week, 1);
        let info = week_in(&terms, day(11, 25)).unwrap();
        assert_eq!(info.week, 11);
        assert!(!info.is_exam_week());
        assert_eq!(info.holiday(day(10, 1)), Some("国庆节"));
        let exam = week_in(&terms, day(1, 17)).unwrap();
        assert_eq!(exam.week, 18);
        assert!(exam.is_exam_week());
        assert!(exam.describe().contains("第 17–18 周为考试周"));
        assert!(week_in(&terms, day(1, 18)).is_none(), "寒假从 1 月 18 日开始");
        assert_eq!(date_of(&info, 11, 3), Some(day(11, 25)));
        assert_eq!(info.term.end(), day(1, 17));
    }

    #[test]
    fn canvas_utc_deadlines_are_shown_in_beijing_time() {
        let now = at(2026, 11, 25, 14, 0);
        let due = parse_time("2026-11-28T15:59:00Z").unwrap();
        assert_eq!(fmt_time(due, now), "11月28日 周六 23:59");
        assert_eq!(fmt_remaining(due, now), "还剩 3 天 9 小时");
        assert_eq!(fmt_remaining(now - chrono::Duration::minutes(90), now), "已过 1 小时 30 分钟");
        assert_eq!(
            describe_deadline("2027-01-05T04:00:00Z", now),
            "2027年1月5日 周二 12:00（还剩 40 天 22 小时）"
        );
        assert_eq!(describe_deadline("not a time", now), "not a time");
    }

    #[test]
    fn periods_follow_the_campus_timetable() {
        let time = |h, m| NaiveTime::from_hms_opt(h, m, 0).unwrap();
        assert_eq!(period_now(time(7, 30)), PeriodNow::Before(1));
        assert_eq!(period_now(time(8, 0)), PeriodNow::InClass(1));
        assert_eq!(period_now(time(9, 45)), PeriodNow::Before(3));
        assert_eq!(period_now(time(14, 10)), PeriodNow::InClass(7));
        assert_eq!(period_now(time(21, 40)), PeriodNow::AfterAll);
        assert_eq!(period_span(1, 2).as_deref(), Some("08:00–09:40"));
        assert_eq!(period_span(11, 13).as_deref(), Some("18:00–20:45"));
        assert_eq!(period_span(0, 2), None);
        assert_eq!(period_span(3, 15), None);
        assert_eq!(academic_term(NaiveDate::from_ymd_opt(2026, 11, 25).unwrap()), (2026, "3", "秋季学期"));
        assert_eq!(academic_term(NaiveDate::from_ymd_opt(2027, 1, 10).unwrap()).0, 2026);
        assert_eq!(academic_term(NaiveDate::from_ymd_opt(2027, 3, 1).unwrap()), (2026, "12", "春季学期"));
    }

    #[test]
    fn describe_now_reports_week_and_period() {
        let text = describe_now(at(2026, 11, 25, 14, 10));
        assert!(text.contains("2026-11-25 14:10:00 周三"), "{text}");
        assert!(text.contains("第 11 周"), "{text}");
        assert!(text.contains("第 7 节（14:00–14:45）"), "{text}");
        let holiday = describe_now(at(2026, 10, 1, 9, 0));
        assert!(holiday.contains("国庆节"), "{holiday}");
        let vacation = describe_now(at(2027, 1, 25, 22, 0));
        assert!(vacation.contains("不在已知学期"), "{vacation}");
    }
}
