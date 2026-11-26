//! Resolve a single Canvas course and one start time before registering a watch.

use std::collections::BTreeSet;

use chrono::{DateTime, Duration, FixedOffset, NaiveDate, NaiveTime, TimeZone, Utc};
use serde_json::Value;

use super::campus_time;
use super::eduinfo::{self, Class, TermCode};
use super::export_calendar::class_events;
use super::get_courses::{Course, active_courses};

pub(super) struct ClassTarget {
    pub course_id: u64,
    pub course_name: String,
    pub start: DateTime<Utc>,
}

// Canvas and the registrar can use different spacing and full-width punctuation.
fn name_key(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_whitespace())
        .map(|c| match c {
            '！'..='～' => char::from_u32(c as u32 - 0xfee0).unwrap(),
            _ => c,
        })
        .collect::<String>()
        .to_lowercase()
}

fn select_course<'a>(
    courses: &'a [Course],
    id: Option<u64>,
    query: &str,
) -> Result<&'a Course, String> {
    let matches_name = |c: &Course| {
        name_key(&c.name).contains(query)
            || c.course_code
                .as_deref()
                .is_some_and(|s| name_key(s).contains(query))
    };
    if let Some(id) = id {
        let course = courses.iter().find(|c| c.id == id).ok_or_else(|| {
            format!("进行中的 Canvas 课程中没有 ID 为 {id} 的课程，可用 get_courses 查找。")
        })?;
        if !query.is_empty() && !matches_name(course) {
            return Err(format!(
                "Canvas 课程 {id} 是「{}」，与指定名称不一致。",
                course.name
            ));
        }
        return Ok(course);
    }
    let mut matches: Vec<_> = courses.iter().filter(|c| matches_name(c)).collect();
    if matches.iter().any(|c| name_key(&c.name) == query) {
        matches.retain(|c| name_key(&c.name) == query);
    }
    match matches.as_slice() {
        [course] => Ok(*course),
        [] => Err("没有匹配的进行中 Canvas 课程，请用 get_courses 核对名称或 ID。".into()),
        _ => {
            let choices = matches
                .iter()
                .map(|c| {
                    format!(
                        "[{}] {} {}",
                        c.id,
                        c.name,
                        c.course_code.as_deref().unwrap_or("")
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            Err(format!(
                "匹配到多个 Canvas 课程，请确认班级并指定 course_id：\n{choices}"
            ))
        }
    }
}

fn select_start(
    classes: &[Class],
    term_start: NaiveDate,
    course: &Course,
    day: Option<NaiveDate>,
    clock: Option<NaiveTime>,
    now: DateTime<FixedOffset>,
) -> Result<DateTime<Utc>, String> {
    let course_name = name_key(&course.name);
    let mut times = BTreeSet::new();
    for class in classes {
        let name = name_key(&class.name);
        if name.is_empty()
            || name != course_name
            || (!class.weeks_text.trim().is_empty() && class.weeks.is_empty())
        {
            continue;
        }
        for event in class_events(std::slice::from_ref(class), term_start) {
            let start = event.start.with_timezone(&campus_time::beijing());
            let matching_day = match day {
                Some(day) => start.date_naive() == day,
                None => start + Duration::minutes(5) >= now,
            };
            if matching_day && clock.is_none_or(|clock| start.time() == clock) {
                times.insert(start);
            }
        }
    }
    let Some(first) = times.first() else {
        return Err(format!(
            "课表中找不到「{}」符合所选时间的堂次。请核对课表，或明确提供上课日期和开始时间（北京时间）。",
            course.name
        ));
    };
    if day.is_some() && times.len() > 1 {
        let options = times
            .iter()
            .map(|t| t.format("%H:%M").to_string())
            .collect::<Vec<_>>()
            .join("、");
        return Err(format!(
            "「{}」当天有多个上课时段（{options}），请指定 start_time。",
            course.name
        ));
    }
    Ok(first.with_timezone(&Utc))
}

pub(super) async fn resolve(args: &Value) -> Result<ClassTarget, String> {
    let now = campus_time::now();
    let string = |key: &str| match &args[key] {
        Value::Null => Ok(None),
        Value::String(s) if !s.trim().is_empty() => Ok(Some(s.trim())),
        _ => Err(format!("{key} 需要非空字符串")),
    };
    let query = name_key(string("course")?.unwrap_or(""));
    let id = match &args["course_id"] {
        Value::Null => None,
        value => Some(
            value
                .as_u64()
                .or_else(|| value.as_str()?.trim().parse().ok())
                .filter(|id| *id > 0)
                .ok_or("course_id 需要正整数")?,
        ),
    };
    if id.is_none() && query.is_empty() {
        return Err("请指定要监看的课程名 course 或 Canvas course_id。".into());
    }
    let day = match string("date")? {
        None => None,
        Some("today" | "今天") => Some(now.date_naive()),
        Some("tomorrow" | "明天") => now.date_naive().succ_opt(),
        Some(date) => Some(
            NaiveDate::parse_from_str(date, "%Y-%m-%d")
                .map_err(|_| "date 应为 YYYY-MM-DD、today 或 tomorrow")?,
        ),
    };
    let clock = string("start_time")?
        .map(|s| NaiveTime::parse_from_str(s, "%H:%M").map_err(|_| "start_time 应为北京时间 HH:MM"))
        .transpose()?;
    let courses = active_courses().await?;
    let course = select_course(&courses, id, &query)?;
    let start = if let (Some(day), Some(clock)) = (day, clock) {
        campus_time::beijing()
            .from_local_datetime(&day.and_time(clock))
            .unwrap()
            .with_timezone(&Utc)
    } else {
        if courses
            .iter()
            .any(|other| other.id != course.id && name_key(&other.name) == name_key(&course.name))
        {
            return Err(format!(
                "多个 Canvas 班级使用「{}」这个课名，无法仅凭教务课名确定班级，请核对并指定日期和开始时间。",
                course.name
            ));
        }
        let info = campus_time::week_of(day.unwrap_or(now.date_naive()))
            .ok_or("该日期不在已知教学周内，请明确提供上课日期和开始时间。")?;
        let term = TermCode::current(info.term.start);
        let results = eduinfo::query(vec![(eduinfo::schedule_request(term), "课表")]).await?;
        let schedule = results.into_iter().next().ok_or("教务系统未返回课表")??;
        select_start(
            &eduinfo::classes_from(&schedule),
            info.term.start,
            course,
            day,
            clock,
            now,
        )?
    };
    Ok(ClassTarget {
        course_id: course.id,
        course_name: course.name.trim().to_string(),
        start,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn separates_parallel_sections_and_multiple_starts_on_one_day() {
        let courses: Vec<Course> = serde_json::from_value(json!([
            {"id":1,"name":"大学物理（A）","course_code":"PHY-01"},
            {"id":2,"name":"大学物理（A）","course_code":"PHY-02"}
        ]))
        .unwrap();
        assert!(select_course(&courses, None, "大学物理").is_err());
        let course = select_course(&courses, Some(2), "大学物理").unwrap();
        let classes = eduinfo::classes_from(&json!({"kbList":[
            {"xqj":"3","jcs":"1-2","zcd":"11-13周(单)","kcmc":"大学物理(A)"},
            {"xqj":"3","jcs":"3-4","zcd":"11-13周(单)","kcmc":"大学物理(A)"}
        ]}));
        let now = campus_time::beijing()
            .with_ymd_and_hms(2026, 11, 25, 8, 6, 0)
            .unwrap();
        let term = campus_time::week_of(now.date_naive()).unwrap().term.start;
        assert!(select_start(&classes, term, course, Some(now.date_naive()), None, now).is_err());
        let next = select_start(&classes, term, course, None, None, now).unwrap();
        assert_eq!(
            next.with_timezone(&campus_time::beijing())
                .format("%F %R")
                .to_string(),
            "2026-11-25 10:00"
        );
    }
}
