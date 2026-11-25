//! 教学信息服务网（i.sjtu.edu.cn，正方教务系统）：课表、考试安排、成绩与绩点。
//!
//! 浏览器先打开 jaccountlogin 完成单点登录，再在页面里 POST 教务系统自己的查询接口，
//! 一次浏览器会话可以查多项。

use std::collections::BTreeSet;
use std::sync::LazyLock;

use chrono::{Datelike, Duration, NaiveDate, NaiveTime};
use regex::Regex;
use serde_json::Value;

use super::campus_time::{self, PERIODS, WeekInfo, date_of, period_span, weekday_cn};
use super::canvas::trim_number;
use super::clip;
use crate::sso::{FetchRequest, FetchResponse, campus_fetch};

const ENTRY: &str = "https://i.sjtu.edu.cn/jaccountlogin";
const SCHEDULE_PATH: &str = "/kbcx/xskbcx_cxXsKb.html?gnmkdm=N2151";
const EXAMS_PATH: &str = "/kwgl/kscx_cxXsksxxIndex.html?doType=query&gnmkdm=N358105";
const SCORES_PATH: &str = "/cjcx/cjcx_cxDgXscj.html?doType=query&gnmkdm=N305005";
const RESPONSE_LIMIT: usize = 2_000_000;

static EXAM_TIME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(\d{4}-\d{2}-\d{2})\s*\(\s*(\d{1,2}:\d{2})\s*-\s*(\d{1,2}:\d{2})\s*\)").unwrap());

/// 正方系统的学年 xnm 与学期 xqm（秋 3、春 12、夏 16）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct TermCode {
    pub xnm: i32,
    pub xqm: &'static str,
}

impl TermCode {
    pub fn current(today: NaiveDate) -> Self {
        let (xnm, xqm, _) = campus_time::academic_term(today);
        Self { xnm, xqm }
    }

    /// “2025-2026-2” → 2025 学年春季学期。
    pub fn parse(text: &str) -> Result<Self, String> {
        let parts: Vec<&str> = text.trim().split('-').map(str::trim).collect();
        let error = || format!("term 格式应为“2025-2026-1”（1 秋季、2 春季、3 夏季），收到「{text}」");
        let [start, end, index] = parts.as_slice() else { return Err(error()) };
        let xnm: i32 = start.parse().map_err(|_| error())?;
        if end.parse::<i32>().ok() != Some(xnm + 1) {
            return Err(error());
        }
        let xqm = match *index {
            "1" => "3",
            "2" => "12",
            "3" => "16",
            _ => return Err(error()),
        };
        Ok(Self { xnm, xqm })
    }

    pub fn season(&self) -> &'static str {
        match self.xqm {
            "3" => "秋季学期",
            "12" => "春季学期",
            _ => "夏季学期",
        }
    }

    pub fn label(&self) -> String {
        format!("{}-{} 学年{}", self.xnm, self.xnm + 1, self.season())
    }

    fn form(&self) -> Vec<(&'static str, String)> {
        vec![("xnm", self.xnm.to_string()), ("xqm", self.xqm.to_string())]
    }
}

fn paged_form(term: Option<TermCode>) -> Vec<(&'static str, String)> {
    let mut form = match term {
        Some(term) => term.form(),
        // 留空表示所有学年学期
        None => vec![("xnm", String::new()), ("xqm", String::new())],
    };
    form.extend([
        ("_search", "false".to_string()),
        ("nd", chrono::Utc::now().timestamp_millis().to_string()),
        ("queryModel.showCount", "500".to_string()),
        ("queryModel.currentPage", "1".to_string()),
        ("queryModel.sortName", String::new()),
        ("queryModel.sortOrder", "asc".to_string()),
        ("time", "0".to_string()),
    ]);
    form
}

fn decode(response: &FetchResponse, what: &str) -> Result<Value, String> {
    if let Some(error) = &response.error {
        return Err(format!("查询{what}失败：{error}"));
    }
    if response.status != 200 {
        return Err(format!("教务系统查询{what}返回 {}", response.status));
    }
    if response.truncated {
        return Err(format!("{what}数据过长，已被截断"));
    }
    serde_json::from_str(&response.text).map_err(|_| {
        // 会话失效时接口会返回登录页 HTML
        format!("教务系统没有返回{what}数据（可能登录已失效或系统维护），请稍后重试")
    })
}

/// 在一次浏览器会话里依次查询多项，按顺序返回。
pub(super) async fn query(requests: Vec<(FetchRequest, &str)>) -> Result<Vec<Result<Value, String>>, String> {
    let (requests, names): (Vec<FetchRequest>, Vec<&str>) = requests.into_iter().unzip();
    let responses = campus_fetch(ENTRY, &requests, RESPONSE_LIMIT).await?;
    Ok(responses.iter().zip(names).map(|(response, name)| decode(response, name)).collect())
}

pub(super) fn schedule_request(term: TermCode) -> FetchRequest {
    let mut form = term.form();
    form.push(("kzlx", "ck".to_string()));
    FetchRequest::post(SCHEDULE_PATH, &form)
}

#[cfg(target_os = "macos")]
pub(super) async fn cached_classes(term: TermCode) -> Result<Vec<Class>, String> {
    let responses = crate::sso::cached_campus_fetch(ENTRY, &[schedule_request(term)], RESPONSE_LIMIT).await?;
    let response = responses.first().ok_or("教务系统未返回课表")?;
    Ok(classes_from(&decode(response, "课表")?))
}

fn text(value: &Value, key: &str) -> String {
    match &value[key] {
        Value::String(text) => text.trim().to_string(),
        Value::Number(number) => number.to_string(),
        _ => String::new(),
    }
}

// ———————————————————————— 课表 ————————————————————————

#[derive(Clone, Debug)]
pub(super) struct Class {
    pub name: String,
    pub teacher: String,
    pub room: String,
    pub weekday: u32,
    pub start: u32,
    pub end: u32,
    pub weeks: BTreeSet<u32>,
    pub weeks_text: String,
    pub credit: String,
    pub assessment: String,
}

/// “1-16周(单)”“3周,5周,10-12周”→ 周次集合。
pub(super) fn parse_weeks(text: &str) -> BTreeSet<u32> {
    let mut weeks = BTreeSet::new();
    for part in text.split([',', '，', ';', '；']) {
        let part = part.trim();
        let odd = part.contains('单');
        let even = part.contains('双');
        let core: String = part.chars().take_while(|c| c.is_ascii_digit() || *c == '-').collect();
        let (first, last) = match core.split_once('-') {
            Some((first, last)) => (first.parse::<u32>(), last.parse::<u32>()),
            None => (core.parse(), core.parse()),
        };
        let (Ok(first), Ok(last)) = (first, last) else { continue };
        for week in first..=last.min(first + 60) {
            if (odd && week % 2 == 0) || (even && week % 2 == 1) {
                continue;
            }
            weeks.insert(week);
        }
    }
    weeks
}

pub(super) fn classes_from(value: &Value) -> Vec<Class> {
    let mut classes: Vec<Class> = value["kbList"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let periods = text(item, "jcs");
            let (start, end) = periods.split_once('-').unwrap_or((periods.as_str(), periods.as_str()));
            let weeks_text = text(item, "zcd");
            let (start, end): (u32, u32) = (start.trim().parse().ok()?, end.trim().parse().ok()?);
            // 节次必须落在作息表里，后面按节次取上下课时间
            if start == 0 || start > end || end as usize > PERIODS.len() {
                return None;
            }
            Some(Class {
                name: text(item, "kcmc"),
                teacher: text(item, "xm"),
                room: text(item, "cdmc"),
                weekday: text(item, "xqj").parse().ok().filter(|day| (1..=7).contains(day))?,
                start,
                end,
                weeks: parse_weeks(&weeks_text),
                weeks_text,
                credit: text(item, "xf"),
                assessment: text(item, "khfsmc"),
            })
        })
        .collect();
    classes.sort_by_key(|class| (class.weekday, class.start));
    classes
}

fn class_line(class: &Class) -> String {
    let mut line = format!(
        "第 {}–{} 节 {} {}",
        class.start,
        class.end,
        period_span(class.start, class.end).unwrap_or_default(),
        class.name
    );
    if !class.room.is_empty() {
        line.push_str(&format!(" @ {}", class.room));
    }
    if !class.teacher.is_empty() {
        line.push_str(&format!(" · {}", class.teacher));
    }
    line
}

/// 某天要上的课（按教学周过滤）。
pub(super) fn classes_on<'a>(classes: &'a [Class], info: &WeekInfo, date: NaiveDate) -> Vec<&'a Class> {
    let weekday = date.weekday().number_from_monday();
    classes
        .iter()
        .filter(|class| class.weekday == weekday && (class.weeks.is_empty() || class.weeks.contains(&info.week)))
        .collect()
}

pub(super) fn render_day(classes: &[Class], date: NaiveDate, now: chrono::DateTime<chrono::FixedOffset>) -> String {
    let prefix = match (date - now.date_naive()).num_days() {
        0 => "今天 ",
        1 => "明天 ",
        _ => "",
    };
    let day = format!("{prefix}{}月{}日 {}", date.month(), date.day(), weekday_cn(date.weekday()));
    let Some(info) = campus_time::week_of(date) else {
        return format!("{day} 不在已知学期的教学周内，没有按周次排的课。可传 day=all 查看整学期课表。");
    };
    let mut text = format!("{day}（第 {} 周", info.week);
    if info.is_exam_week() {
        text.push_str("，考试周");
    }
    text.push('）');
    let holiday = info.holiday(date);
    let today = classes_on(classes, &info, date);
    if today.is_empty() {
        text.push_str(&match holiday {
            Some(holiday) => format!("：{holiday}放假，没有课。\n"),
            None => "：没有课。\n".to_string(),
        });
        return text;
    }
    text.push_str(&format!("：共 {} 节课", today.len()));
    if let Some(holiday) = holiday {
        text.push_str(&format!("（{holiday}放假，按校历停课或另行调课）"));
    }
    text.push('\n');
    let is_today = date == now.date_naive();
    for class in today {
        let mut line = format!("- {}", class_line(class));
        if is_today {
            let end = NaiveTime::parse_from_str(PERIODS[class.end as usize - 1].1, "%H:%M").ok();
            let start = NaiveTime::parse_from_str(PERIODS[class.start as usize - 1].0, "%H:%M").ok();
            match (start, end) {
                (_, Some(end)) if now.time() >= end => line.push_str("（已结束）"),
                (Some(start), _) if now.time() >= start => line.push_str("（正在上）"),
                _ => {}
            }
        }
        text.push_str(&line);
        text.push('\n');
    }
    text
}

pub(super) fn render_week(classes: &[Class], info: &WeekInfo, week: u32) -> String {
    let mut text = format!("{} 第 {week} 周课表", info.term.name);
    if week + info.term.exam_weeks > info.term.weeks {
        text.push_str("（考试周）");
    }
    text.push('\n');
    let shifted = WeekInfo { term: info.term.clone(), week };
    let mut total = 0;
    for weekday in 1..=7 {
        let Some(date) = date_of(info, week, weekday) else { continue };
        let day = classes_on(classes, &shifted, date);
        if day.is_empty() {
            continue;
        }
        total += day.len();
        text.push_str(&format!("\n{} {}月{}日", weekday_cn(date.weekday()), date.month(), date.day()));
        if let Some(holiday) = shifted.holiday(date) {
            text.push_str(&format!("（{holiday}放假）"));
        }
        text.push('\n');
        for class in day {
            text.push_str(&format!("- {}\n", class_line(class)));
        }
    }
    if total == 0 {
        text.push_str("这一周没有课。\n");
    }
    text
}

pub(super) fn render_all(classes: &[Class], term: TermCode) -> String {
    if classes.is_empty() {
        return format!("{} 的课表是空的（可能还未排课）。", term.label());
    }
    let mut text = format!("{} 课表（{} 个上课时段）\n", term.label(), classes.len());
    let mut seen: Vec<&str> = Vec::new();
    for class in classes {
        text.push_str(&format!(
            "- {} {} · {}",
            weekday_cn(chrono::Weekday::try_from(class.weekday as u8 - 1).unwrap_or(chrono::Weekday::Mon)),
            class_line(class),
            class.weeks_text
        ));
        if !seen.contains(&class.name.as_str()) {
            seen.push(&class.name);
            let mut facts = Vec::new();
            if !class.credit.is_empty() {
                facts.push(format!("{} 学分", class.credit));
            }
            if !class.assessment.is_empty() {
                facts.push(class.assessment.clone());
            }
            if !facts.is_empty() {
                text.push_str(&format!(" · {}", facts.join("，")));
            }
        }
        text.push('\n');
    }
    text
}

fn term_argument(arguments: &Value, today: NaiveDate) -> Result<(TermCode, bool), String> {
    let current = TermCode::current(today);
    match arguments["term"].as_str().map(str::trim).filter(|text| !text.is_empty()) {
        None => Ok((current, true)),
        Some(text) => {
            let term = TermCode::parse(text)?;
            Ok((term, term == current))
        }
    }
}

pub(super) async fn class_schedule(arguments: Value) -> Result<String, String> {
    let now = campus_time::now();
    let today = now.date_naive();
    let (term, is_current) = term_argument(&arguments, today)?;
    let day = arguments["day"].as_str().map(str::trim).filter(|text| !text.is_empty()).unwrap_or("today");
    let week = arguments["week"].as_u64().map(|week| week as u32);
    // 先校验参数，再启动浏览器
    let date = match day {
        "today" | "今天" => Some(today),
        "tomorrow" | "明天" => Some(today + Duration::days(1)),
        "this_week" | "next_week" | "all" => None,
        other => Some(
            NaiveDate::parse_from_str(other, "%Y-%m-%d")
                .map_err(|_| format!("day 应为 today、tomorrow、this_week、next_week、all 或 YYYY-MM-DD，收到「{other}」"))?,
        ),
    };
    if week.is_some_and(|week| !(1..=30).contains(&week)) {
        return Err("week 应为 1 到 30 的教学周".into());
    }

    let mut results = query(vec![(schedule_request(term), "课表")]).await?;
    let classes = classes_from(&results.remove(0)?);
    if classes.is_empty() {
        return Ok(format!("{} 的课表是空的（可能还未排课）。", term.label()));
    }
    let current = campus_time::week_of(today);
    let text = match (is_current, week, day, &current) {
        (false, _, _, _) | (_, _, "all", _) => render_all(&classes, term),
        (true, Some(week), _, Some(info)) => render_week(&classes, info, week),
        (true, None, "this_week", Some(info)) => render_week(&classes, info, info.week),
        (true, None, "next_week", Some(info)) => render_week(&classes, info, info.week + 1),
        (true, None, _, _) if date.is_some() => render_day(&classes, date.unwrap_or(today), now),
        // 假期里没有当前教学周，只能给整学期视图
        _ => format!("当前不在教学周内。\n{}", render_all(&classes, term)),
    };
    Ok(clip(&text))
}

// ———————————————————————— 考试 ————————————————————————

pub(super) struct Exam {
    pub course: String,
    pub name: String,
    pub when: Option<(NaiveDate, String, String)>,
    pub room: String,
    pub campus: String,
    pub raw_time: String,
}

pub(super) fn exams_from(value: &Value) -> Vec<Exam> {
    let mut exams: Vec<Exam> = value["items"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|item| {
            let raw_time = text(item, "kssj");
            let when = EXAM_TIME.captures(&raw_time).and_then(|caps| {
                Some((NaiveDate::parse_from_str(&caps[1], "%Y-%m-%d").ok()?, caps[2].to_string(), caps[3].to_string()))
            });
            Exam {
                course: text(item, "kcmc"),
                name: text(item, "ksmc"),
                when,
                room: text(item, "cdmc"),
                campus: text(item, "cdxqmc"),
                raw_time,
            }
        })
        .collect();
    exams.sort_by(|a, b| match (&a.when, &b.when) {
        (Some(a), Some(b)) => (a.0, &a.1).cmp(&(b.0, &b.1)),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => a.course.cmp(&b.course),
    });
    exams
}

fn unreleased(text: &str) -> bool {
    text.is_empty() || text.contains("未放开") || text.contains("不可查")
}

pub(super) fn exam_line(exam: &Exam, today: NaiveDate) -> String {
    let mut line = match &exam.when {
        Some((date, start, end)) => {
            let days = (*date - today).num_days();
            let countdown = match days {
                0 => "今天".to_string(),
                1 => "明天".to_string(),
                days if days > 0 => format!("还有 {days} 天"),
                _ => "已考完".to_string(),
            };
            format!(
                "{}月{}日 {} {start}–{end}（{countdown}）· {}",
                date.month(),
                date.day(),
                weekday_cn(date.weekday()),
                exam.course
            )
        }
        None if unreleased(&exam.raw_time) => format!("时间未公布 · {}", exam.course),
        None => format!("{} · {}", exam.raw_time, exam.course),
    };
    if !exam.name.is_empty() {
        line.push_str(&format!(" · {}", exam.name));
    }
    if unreleased(&exam.room) {
        line.push_str(" · 考场未公布");
    } else {
        line.push_str(&format!(" · {}", exam.room));
        if !exam.campus.is_empty() && !unreleased(&exam.campus) {
            line.push_str(&format!("（{}）", exam.campus));
        }
    }
    line
}

pub(super) fn render_exams(exams: &[Exam], term: TermCode, today: NaiveDate) -> String {
    if exams.is_empty() {
        return format!("{} 还没有考试安排。期末考试通常在第 17–18 周，教务处公布后可再查询。", term.label());
    }
    let mut text = format!("{} 考试安排（{} 场，按时间排序）\n", term.label(), exams.len());
    for exam in exams {
        text.push_str(&format!("- {}\n", exam_line(exam, today)));
    }
    if let Some((date, _, _)) = exams.iter().filter_map(|exam| exam.when.as_ref()).find(|(date, _, _)| *date >= today) {
        text.push_str(&format!("\n离最近的一场考试还有 {} 天。", (*date - today).num_days()));
    }
    if exams.iter().any(|exam| exam.when.is_none()) {
        text.push_str("\n部分考试的时间或考场尚未公布，以教学信息服务网最新通知为准。");
    }
    text
}

pub(super) fn exams_request(term: TermCode) -> FetchRequest {
    FetchRequest::post(EXAMS_PATH, &paged_form(Some(term)))
}

pub(super) async fn exam_schedule(arguments: Value) -> Result<String, String> {
    let today = campus_time::now().date_naive();
    let (term, _) = term_argument(&arguments, today)?;
    let mut results = query(vec![(exams_request(term), "考试安排")]).await?;
    let exams = exams_from(&results.remove(0)?);
    Ok(clip(&render_exams(&exams, term, today)))
}

// ———————————————————————— 成绩 ————————————————————————

pub(super) struct Score {
    pub course: String,
    pub code: String,
    pub credit: Option<f64>,
    pub grade: String,
    pub point: Option<f64>,
    pub percent: Option<f64>,
    pub term: (i32, u32),
    pub term_label: String,
    pub nature: String,
    pub invalid: bool,
}

fn term_order(xqm: &str) -> u32 {
    match xqm {
        "3" => 1,
        "12" => 2,
        "16" => 3,
        _ => 0,
    }
}

pub(super) fn scores_from(value: &Value) -> Vec<Score> {
    value["items"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|item| {
            let xqm = text(item, "xqm");
            let season = match xqm.as_str() {
                "3" => "秋季学期",
                "12" => "春季学期",
                "16" => "夏季学期",
                _ => "",
            };
            Score {
                course: text(item, "kcmc"),
                code: text(item, "kch"),
                credit: text(item, "xf").parse().ok(),
                grade: text(item, "cj"),
                point: text(item, "jd").parse().ok(),
                percent: text(item, "bfzcj").parse().ok(),
                term: (text(item, "xnm").parse().unwrap_or(0), term_order(&xqm)),
                term_label: format!("{} 学年{season}", text(item, "xnmmc")),
                nature: text(item, "ksxz"),
                invalid: text(item, "cjsfzf") == "是",
            }
        })
        .collect()
}

#[derive(Default, Debug, PartialEq)]
pub(super) struct Gpa {
    pub courses: usize,
    pub credits: f64,
    pub gpa: Option<f64>,
    pub average: Option<f64>,
    pub pass_fail: usize,
}

/// 按学分加权：绩点为空的（P/F 等）不计入绩点，但计入已获学分。
pub(super) fn gpa<'a>(scores: impl Iterator<Item = &'a Score>) -> Gpa {
    let mut result = Gpa::default();
    let (mut point_sum, mut point_credits, mut percent_sum, mut percent_credits) = (0.0, 0.0, 0.0, 0.0);
    for score in scores.filter(|score| !score.invalid) {
        let credit = score.credit.unwrap_or(0.0);
        result.courses += 1;
        result.credits += credit;
        match score.point {
            Some(point) => {
                point_sum += point * credit;
                point_credits += credit;
                if let Some(percent) = score.percent {
                    percent_sum += percent * credit;
                    percent_credits += credit;
                }
            }
            None => result.pass_fail += 1,
        }
    }
    result.gpa = (point_credits > 0.0).then(|| point_sum / point_credits);
    result.average = (percent_credits > 0.0).then(|| percent_sum / percent_credits);
    result
}

fn gpa_text(stats: &Gpa) -> String {
    let mut text = format!("{} 门 · {} 学分", stats.courses, trim_number(stats.credits));
    if let Some(gpa) = stats.gpa {
        text.push_str(&format!(" · 加权绩点 {gpa:.2}"));
    }
    if let Some(average) = stats.average {
        text.push_str(&format!(" · 加权均分 {average:.1}"));
    }
    if stats.pass_fail > 0 {
        text.push_str(&format!("（{} 门 P/F 等无绩点课程不计入绩点）", stats.pass_fail));
    }
    text
}

pub(super) fn render_scores(scores: &mut [Score], only: Option<TermCode>) -> String {
    if scores.is_empty() {
        return match only {
            Some(term) => format!("{} 还没有出成绩。", term.label()),
            None => "教务系统里还没有成绩记录。".into(),
        };
    }
    scores.sort_by(|a, b| b.term.cmp(&a.term).then(a.course.cmp(&b.course)));
    let title = only.map_or("全部学期".to_string(), |term| term.label());
    let mut text = format!("成绩与绩点（教务系统，{title}）\n总计：{}\n", gpa_text(&gpa(scores.iter())));
    let mut codes: Vec<&str> = scores.iter().map(|score| score.code.as_str()).filter(|code| !code.is_empty()).collect();
    codes.sort_unstable();
    let retakes = codes.windows(2).any(|pair| pair[0] == pair[1]);

    let mut index = 0;
    while index < scores.len() {
        let term = scores[index].term;
        let group: Vec<&Score> = scores[index..].iter().take_while(|score| score.term == term).collect();
        index += group.len();
        text.push_str(&format!("\n{}：{}\n", group[0].term_label, gpa_text(&gpa(group.iter().copied()))));
        for score in group {
            let mut line = format!("- {}", score.course);
            if let Some(credit) = score.credit {
                line.push_str(&format!(" · {} 学分", trim_number(credit)));
            }
            line.push_str(&format!(" · 成绩 {}", if score.grade.is_empty() { "—" } else { &score.grade }));
            if let Some(point) = score.point {
                line.push_str(&format!(" · 绩点 {}", trim_number(point)));
            }
            if !score.nature.is_empty() && score.nature != "正常考试" {
                line.push_str(&format!(" · {}", score.nature));
            }
            if score.invalid {
                line.push_str(" · 成绩已作废");
            }
            text.push_str(&line);
            text.push('\n');
        }
    }
    text.push_str("\n绩点按教务系统给出的每门课绩点、以学分加权计算");
    if retakes {
        text.push_str("；存在同一课程多条记录（补考/重修），这里逐条计入");
    }
    text.push_str("。官方 GPA 以教务处成绩单为准。");
    text
}

pub(super) async fn score_report(arguments: Value) -> Result<String, String> {
    let only = match arguments["term"].as_str().map(str::trim).filter(|text| !text.is_empty()) {
        Some(text) => Some(TermCode::parse(text)?),
        None => None,
    };
    let request = FetchRequest::post(SCORES_PATH, &paged_form(only));
    let mut results = query(vec![(request, "成绩")]).await?;
    let value = results.remove(0)?;
    let mut scores = scores_from(&value);
    let mut text = render_scores(&mut scores, only);
    if let Some(total) = value["totalCount"].as_u64().filter(|total| *total as usize > scores.len()) {
        text.push_str(&format!("\n（共有 {total} 条记录，只取到前 {} 条）", scores.len()));
    }
    Ok(clip(&text))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use serde_json::json;

    fn now(d: u32, h: u32) -> chrono::DateTime<chrono::FixedOffset> {
        campus_time::beijing().with_ymd_and_hms(2026, 11, d, h, 0, 0).unwrap()
    }

    fn schedule() -> Value {
        json!({"kbList": [
            {"kcmc": "大学物理", "xm": "张老师", "cdmc": "东上院102", "jcs": "3-4", "xqj": "3", "zcd": "1-16周", "xf": "4", "khfsmc": "考试"},
            {"kcmc": "概率统计", "xm": "李老师", "cdmc": "东中院3-104", "jcs": "1-2", "xqj": 3, "zcd": "1-15周(单)", "xf": "3"},
            {"kcmc": "形势与政策", "xm": "", "cdmc": "", "jcs": "11-12", "xqj": "4", "zcd": "5周,8周,11周"},
            {"kcmc": "坏数据", "jcs": "", "xqj": "9", "zcd": ""},
            {"kcmc": "越界节次", "jcs": "13-15", "xqj": "2", "zcd": "1-16周"},
            {"kcmc": "零节次", "jcs": "0-2", "xqj": "2", "zcd": "1-16周"}
        ]})
    }

    #[test]
    fn weeks_and_terms_are_parsed_like_the_registrar_writes_them() {
        assert_eq!(parse_weeks("1-16周").len(), 16);
        assert_eq!(parse_weeks("1-15周(单)").into_iter().collect::<Vec<_>>(), vec![1, 3, 5, 7, 9, 11, 13, 15]);
        assert_eq!(parse_weeks("2-8周(双),11周").into_iter().collect::<Vec<_>>(), vec![2, 4, 6, 8, 11]);
        assert_eq!(parse_weeks("5周,8周,11周").into_iter().collect::<Vec<_>>(), vec![5, 8, 11]);
        assert!(parse_weeks("").is_empty());
        assert_eq!(TermCode::parse("2025-2026-2"), Ok(TermCode { xnm: 2025, xqm: "12" }));
        assert!(TermCode::parse("2025-2027-1").is_err());
        assert!(TermCode::parse("2025春").is_err());
        assert_eq!(TermCode::current(NaiveDate::from_ymd_opt(2026, 11, 25).unwrap()).label(), "2026-2027 学年秋季学期");
    }

    #[test]
    fn day_view_filters_by_teaching_week_and_marks_progress() {
        let classes = classes_from(&schedule());
        assert_eq!(classes.len(), 3, "无法解析的条目被跳过");
        // 11月25日周三是第 11 周（单周）：两门课都上；10:30 时物理正在上
        let today = render_day(&classes, NaiveDate::from_ymd_opt(2026, 11, 25).unwrap(), now(25, 10) + Duration::minutes(30));
        assert!(today.contains("今天 11月25日 周三（第 11 周）：共 2 节课"), "{today}");
        assert!(today.contains("- 第 1–2 节 08:00–09:40 概率统计 @ 东中院3-104 · 李老师（已结束）"), "{today}");
        assert!(today.contains("- 第 3–4 节 10:00–11:40 大学物理 @ 东上院102 · 张老师（正在上）"), "{today}");
        // 12月2日是第 12 周（双周），单周的概率统计不上
        let later = render_day(&classes, NaiveDate::from_ymd_opt(2026, 12, 2).unwrap(), now(25, 10));
        assert!(later.contains("共 1 节课") && !later.contains("概率统计"), "{later}");
        let tomorrow = render_day(&classes, NaiveDate::from_ymd_opt(2026, 11, 26).unwrap(), now(25, 10));
        assert!(tomorrow.contains("明天 11月26日 周四（第 11 周）：共 1 节课") && tomorrow.contains("形势与政策"), "{tomorrow}");
        let holiday = render_day(&classes, NaiveDate::from_ymd_opt(2026, 10, 1).unwrap(), now(25, 10));
        assert!(holiday.contains("国庆节放假"), "{holiday}");
    }

    #[test]
    fn week_and_term_views() {
        let classes = classes_from(&schedule());
        let info = campus_time::week_of(NaiveDate::from_ymd_opt(2026, 11, 25).unwrap()).unwrap();
        let week = render_week(&classes, &info, 11);
        assert!(week.contains("第 11 周课表"), "{week}");
        assert!(week.find("周三 11月25日").unwrap() < week.find("周四 11月26日").unwrap(), "{week}");
        let all = render_all(&classes, TermCode { xnm: 2026, xqm: "3" });
        assert!(all.contains("2026-2027 学年秋季学期 课表（3 个上课时段）"), "{all}");
        assert!(all.contains("- 周三 第 1–2 节 08:00–09:40 概率统计 @ 东中院3-104 · 李老师 · 1-15周(单) · 3 学分"), "{all}");
        assert!(all.contains("1-16周 · 4 学分，考试"), "{all}");
    }

    #[test]
    fn exams_sort_by_time_and_flag_unreleased_rooms() {
        let exams = exams_from(&json!({"items": [
            {"kcmc": "大学物理", "ksmc": "2026-2027-1期末考试", "kssj": "未放开不可查", "cdmc": "未放开不可查", "cdxqmc": "未放开不可查"},
            {"kcmc": "概率统计", "ksmc": "2026-2027-1期末考试", "kssj": "2027-01-08(13:00-15:00)", "cdmc": "东上院102", "cdxqmc": "闵行"},
            {"kcmc": "物理", "ksmc": "期中考试", "kssj": "2026-11-20(19:00-21:00)", "cdmc": "东中院1-101", "cdxqmc": "闵行"}
        ]}));
        let text = render_exams(&exams, TermCode { xnm: 2026, xqm: "3" }, NaiveDate::from_ymd_opt(2026, 11, 25).unwrap());
        let mid = text.find("- 11月20日 周五 19:00–21:00（已考完）· 物理 · 期中考试 · 东中院1-101（闵行）").unwrap();
        let fin = text.find("- 1月8日 周五 13:00–15:00（还有 44 天）· 概率统计").unwrap();
        let unknown = text.find("- 时间未公布 · 大学物理 · 2026-2027-1期末考试 · 考场未公布").unwrap();
        assert!(mid < fin && fin < unknown, "{text}");
        assert!(text.contains("离最近的一场考试还有 44 天"), "{text}");
    }

    #[test]
    fn gpa_is_credit_weighted_and_skips_pass_fail_and_void_records() {
        let mut scores = scores_from(&json!({"items": [
            {"kcmc": "大学物理", "kch": "PHY1", "xf": "4", "cj": "93", "jd": "4.00", "bfzcj": "93", "xnm": "2025", "xnmmc": "2025-2026", "xqm": "12", "ksxz": "正常考试"},
            {"kcmc": "高等数学", "kch": "MATH1", "xf": "6", "cj": "B+", "jd": "3.30", "bfzcj": "82", "xnm": "2025", "xnmmc": "2025-2026", "xqm": "3", "ksxz": "正常考试"},
            {"kcmc": "体育", "kch": "PE1", "xf": "1", "cj": "P", "jd": null, "bfzcj": "85", "xnm": "2025", "xnmmc": "2025-2026", "xqm": "3"},
            {"kcmc": "作废课", "kch": "X", "xf": "2", "cj": "60", "jd": "1.00", "xnm": "2025", "xqm": "3", "cjsfzf": "是"}
        ]}));
        let all = gpa(scores.iter());
        assert_eq!((all.courses, all.pass_fail), (3, 1));
        assert_eq!(all.credits, 11.0);
        assert!((all.gpa.unwrap() - 3.58).abs() < 1e-9, "{:?}", all.gpa);
        assert!((all.average.unwrap() - 86.4).abs() < 1e-9, "{:?}", all.average);
        let text = render_scores(&mut scores, None);
        assert!(text.contains("总计：3 门 · 11 学分 · 加权绩点 3.58 · 加权均分 86.4（1 门 P/F 等无绩点课程不计入绩点）"), "{text}");
        let spring = text.find("2025-2026 学年春季学期：1 门 · 4 学分 · 加权绩点 4.00").unwrap();
        let fall = text.find("2025-2026 学年秋季学期：").unwrap();
        assert!(spring < fall, "新学期在前：{text}");
        assert!(text.contains("- 高等数学 · 6 学分 · 成绩 B+ · 绩点 3.3"), "{text}");
        assert!(text.contains("- 作废课 · 2 学分 · 成绩 60 · 绩点 1 · 成绩已作废"), "{text}");
    }
}
