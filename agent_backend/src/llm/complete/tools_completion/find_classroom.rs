//! find_classroom：找空教室/自习室。数据来自学校公开的教室信息服务（ids.sjtu.edu.cn），
//! 无需登录：每间教室当天的排课和传感器统计的实时人数。

use std::collections::BTreeSet;

use chrono::{Datelike, Duration, NaiveDate};
use futures_util::future::join_all;
use serde_json::Value;

use super::campus_time::{self, PERIODS, PeriodNow, period_now, period_span, weekday_cn};
use super::canvas::null_default;
use super::clip;

const IDS_BASE: &str = "https://ids.sjtu.edu.cn";
const MAX_ROOMS: usize = 30;
const LAST_PERIOD: u32 = PERIODS.len() as u32;

#[derive(serde::Deserialize)]
struct Reply<T> {
    #[serde(default)]
    code: i64,
    #[serde(default)]
    msg: Option<String>,
    data: Option<T>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct AreaData {
    #[serde(default, deserialize_with = "null_default")]
    build_list: Vec<Area>,
}

#[derive(serde::Deserialize)]
struct Area {
    #[serde(default)]
    name: String,
    #[serde(default, deserialize_with = "null_default")]
    children: Vec<Building>,
}

#[derive(serde::Deserialize, Clone)]
pub(super) struct Building {
    #[serde(default)]
    name: String,
    id: u64,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct FloorData {
    #[serde(default, deserialize_with = "null_default")]
    floor_list: Vec<Floor>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct Floor {
    #[serde(default, deserialize_with = "null_default")]
    room_stu_numbs: Vec<RoomCount>,
    #[serde(default, deserialize_with = "null_default")]
    children: Vec<Room>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct RoomCount {
    room_id: u64,
    #[serde(default)]
    actual_stu_num: Option<i64>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct Room {
    id: u64,
    #[serde(default)]
    name: String,
    #[serde(default, deserialize_with = "null_default")]
    room_course_list: Vec<RoomCourse>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct RoomCourse {
    #[serde(default)]
    course_name: String,
    #[serde(default)]
    teacher_name: Option<String>,
    #[serde(default)]
    start_section: u32,
    #[serde(default)]
    end_section: u32,
    #[serde(default)]
    assigned_weeks: Option<String>,
    #[serde(default)]
    all_weeks: Option<String>,
}

impl RoomCourse {
    /// assignedWeeks 形如 "1,2,3"，allWeeks 形如 "#1#2#3#"；都没有时视为每周都上。
    fn runs_in(&self, week: Option<u32>) -> bool {
        let Some(week) = week else { return true };
        let weeks = self
            .assigned_weeks
            .as_deref()
            .filter(|text| !text.trim().is_empty())
            .or(self.all_weeks.as_deref())
            .unwrap_or_default();
        let listed: Vec<u32> = weeks
            .split([',', '#'])
            .filter_map(|part| part.trim().parse().ok())
            .collect();
        listed.is_empty() || listed.contains(&week)
    }
}

/// 一间教室在当天的占用情况。
struct RoomDay<'a> {
    name: &'a str,
    busy: BTreeSet<u32>,
    courses: Vec<&'a RoomCourse>,
    people: Option<i64>,
}

impl RoomDay<'_> {
    fn free_ranges(&self, from: u32, to: u32) -> Vec<(u32, u32)> {
        let mut ranges = Vec::new();
        let mut start = None;
        for period in from..=to {
            match (self.busy.contains(&period), start) {
                (false, None) => start = Some(period),
                (true, Some(first)) => {
                    ranges.push((first, period - 1));
                    start = None;
                }
                _ => {}
            }
        }
        if let Some(first) = start {
            ranges.push((first, to));
        }
        ranges
    }

    /// 从 from 节开始连续空闲到第几节；from 节本身有课时为 None。
    fn free_until(&self, from: u32, to: u32) -> Option<u32> {
        (from..=to).take_while(|period| !self.busy.contains(period)).last()
    }
}

fn ranges_text(ranges: &[(u32, u32)]) -> String {
    ranges
        .iter()
        .map(|(start, end)| {
            let span = period_span(*start, *end).unwrap_or_default();
            if start == end {
                format!("第 {start} 节（{span}）")
            } else {
                format!("第 {start}–{end} 节（{span}）")
            }
        })
        .collect::<Vec<_>>()
        .join("、")
}

pub(super) struct Query {
    pub date: NaiveDate,
    pub is_today: bool,
    pub start: u32,
    pub end: u32,
    /// 用户明确给了时间段：只要整段都空的教室
    pub whole_range: bool,
    pub room: Option<String>,
}

/// 一栋楼在某天的分析结果：每间教室的占用，以及符合条件的空教室（下标、空到第几节、空闲节数）。
struct Analysis<'a> {
    header: String,
    rooms: Vec<RoomDay<'a>>,
    free: Vec<(usize, u32, usize)>,
}

fn analyze<'a>(building: &str, data: &'a FloorData, query: &Query) -> Analysis<'a> {
    let week = campus_time::week_of(query.date);
    let mut rooms: Vec<RoomDay> = Vec::new();
    for floor in &data.floor_list {
        for room in &floor.children {
            let courses: Vec<&RoomCourse> = room
                .room_course_list
                .iter()
                .filter(|course| course.runs_in(week.as_ref().map(|info| info.week)))
                .collect();
            let busy = courses
                .iter()
                .flat_map(|course| course.start_section..=course.end_section.max(course.start_section))
                .collect();
            let people = floor
                .room_stu_numbs
                .iter()
                .find(|count| count.room_id == room.id)
                .and_then(|count| count.actual_stu_num);
            rooms.push(RoomDay { name: room.name.trim(), busy, courses, people });
        }
    }

    let mut header = format!(
        "{building} · {}月{}日 {}",
        query.date.month(),
        query.date.day(),
        weekday_cn(query.date.weekday())
    );
    match &week {
        Some(info) => {
            header.push_str(&format!("（第 {} 周", info.week));
            if let Some(holiday) = info.holiday(query.date) {
                header.push_str(&format!("，{holiday}放假"));
            }
            header.push('）');
        }
        None => header.push_str("（不在已知教学周内，按全部排课计算，结果偏保守）"),
    }
    header.push('\n');

    let mut free: Vec<(usize, u32, usize)> = Vec::new();
    if query.start <= query.end {
        for (index, room) in rooms.iter().enumerate() {
            let ranges = room.free_ranges(query.start, query.end);
            let total = ranges.iter().map(|(start, end)| (end - start + 1) as usize).sum::<usize>();
            if query.whole_range {
                if total == (query.end - query.start + 1) as usize {
                    free.push((index, query.end, total));
                }
            } else if let Some(until) = room.free_until(query.start, query.end) {
                free.push((index, until, total));
            }
        }
    }
    free.sort_by(|a, b| {
        let (left, right) = (&rooms[a.0], &rooms[b.0]);
        b.1.cmp(&a.1)
            .then(b.2.cmp(&a.2))
            .then(left.people.unwrap_or(0).cmp(&right.people.unwrap_or(0)))
            .then(left.name.cmp(right.name))
    });
    Analysis { header, rooms, free }
}

fn render_room(analysis: &Analysis, wanted: &str, query: &Query) -> String {
    let mut text = analysis.header.clone();
    let wanted: String = wanted.chars().filter(|c| !c.is_whitespace()).collect();
    let Some(room) = analysis.rooms.iter().find(|room| room.name.replace(' ', "") == wanted) else {
        let names: Vec<&str> = analysis.rooms.iter().map(|room| room.name).take(60).collect();
        return format!("{text}没有找到教室「{wanted}」。该楼的教室有：{}", names.join("、"));
    };
    text.push_str(&format!("{} 当天排课：\n", room.name));
    let mut courses = room.courses.clone();
    courses.sort_by_key(|course| course.start_section);
    if courses.is_empty() {
        text.push_str("- 全天没有课\n");
    }
    for course in courses {
        text.push_str(&format!(
            "- 第 {}–{} 节（{}）{}{}\n",
            course.start_section,
            course.end_section,
            period_span(course.start_section, course.end_section).unwrap_or_default(),
            course.course_name.trim(),
            course.teacher_name.as_deref().map(|name| format!("，{}", name.trim())).unwrap_or_default()
        ));
    }
    let free = room.free_ranges(1, LAST_PERIOD);
    if !free.is_empty() {
        text.push_str(&format!("空闲时段：{}\n", ranges_text(&free)));
    }
    if query.is_today {
        if let Some(people) = room.people {
            text.push_str(&format!("实时人数：{people}\n"));
        }
    }
    text
}

pub(super) fn render_building(building: &str, data: &FloorData, query: &Query) -> String {
    let analysis = analyze(building, data, query);
    if let Some(wanted) = &query.room {
        return render_room(&analysis, wanted, query);
    }
    let mut text = analysis.header.clone();
    if query.start > query.end {
        text.push_str("今天的上课时段已经结束（第 14 节 21:40 下课），教学楼可能已关闭，可以查明天的空教室。");
        return text;
    }
    if query.whole_range {
        text.push_str(&format!(
            "第 {}–{} 节（{}）全程空闲的教室 {} 间：\n",
            query.start,
            query.end,
            period_span(query.start, query.end).unwrap_or_default(),
            analysis.free.len()
        ));
    } else {
        text.push_str(&format!(
            "从第 {} 节起空闲的教室 {} 间（共 {} 间，按连续空闲时长排序）：\n",
            query.start,
            analysis.free.len(),
            analysis.rooms.len()
        ));
    }
    for (index, until, _) in analysis.free.iter().take(MAX_ROOMS) {
        let room = &analysis.rooms[*index];
        let mut line = format!("- {}：", room.name);
        if query.whole_range {
            line.push_str("整段空闲");
        } else if *until >= query.end {
            line.push_str(if query.end == LAST_PERIOD { "之后都没有课" } else { "整段空闲" });
        } else {
            line.push_str(&format!("空到第 {until} 节（{} 起有课）", PERIODS[*until as usize].0));
        }
        if query.is_today {
            if let Some(people) = room.people {
                line.push_str(&format!(" · 实时 {people} 人"));
            }
        }
        text.push_str(&line);
        text.push('\n');
    }
    if analysis.free.len() > MAX_ROOMS {
        text.push_str(&format!("另有 {} 间未列出。\n", analysis.free.len() - MAX_ROOMS));
    }
    if analysis.free.is_empty() {
        text.push_str("这段时间没有空教室，可以换个楼或时间段。\n");
    }
    text.push_str("排课来自学校教室系统，临时借用、考试和讲座不一定显示");
    text.push_str(if query.is_today { "；人数来自教室传感器，仅供参考。" } else { "。" });
    text
}

async fn post<T: serde::de::DeserializeOwned>(path: &str, form: &[(&str, String)]) -> Result<T, String> {
    let reply: Reply<T> = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|err| err.to_string())?
        .post(format!("{IDS_BASE}{path}"))
        .form(form)
        .send()
        .await
        .and_then(|response| response.error_for_status())
        .map_err(|err| format!("访问教室信息服务失败: {err}"))?
        .json()
        .await
        .map_err(|err| format!("教室信息服务返回格式变化: {err}"))?;
    if reply.code != 200 {
        return Err(format!(
            "教室信息服务返回错误：{}",
            reply.msg.unwrap_or_else(|| reply.code.to_string())
        ));
    }
    reply.data.ok_or_else(|| "教室信息服务没有返回数据".to_string())
}

async fn buildings() -> Result<Vec<(String, Building)>, String> {
    let data: AreaData = post("/build/findAreaBuild", &[]).await?;
    Ok(data
        .build_list
        .into_iter()
        .flat_map(|area| {
            let campus = area.name.clone();
            area.children.into_iter().map(move |building| (campus.clone(), building))
        })
        .collect())
}

/// “东上院102” → (东上院, Some(东上院102))；按最长匹配，避免“东上院”被认成“上院”。
fn match_building<'a>(list: &'a [(String, Building)], wanted: &str) -> Option<(&'a Building, Option<String>)> {
    let wanted: String = wanted.chars().filter(|c| !c.is_whitespace()).collect();
    let wanted = wanted.replace("陈瑞球楼", "陈瑞球").replace("陈瑞球", "陈瑞球楼");
    let building = list
        .iter()
        .map(|(_, building)| building)
        .filter(|building| wanted.starts_with(building.name.as_str()) || building.name.starts_with(&wanted))
        .max_by_key(|building| {
            // 完整包含楼名的优先，其次是用户只写了楼名前缀（如“东上”）
            (wanted.starts_with(building.name.as_str()), building.name.chars().count())
        })?;
    let rest = wanted.strip_prefix(building.name.as_str()).unwrap_or_default();
    let room = rest
        .chars()
        .any(|c| c.is_ascii_digit())
        .then(|| format!("{}{}", building.name, rest.trim_start_matches(['-', '_'])));
    Some((building, room))
}

fn parse_date(arguments: &Value, today: NaiveDate) -> Result<NaiveDate, String> {
    match arguments["date"].as_str().map(str::trim).filter(|text| !text.is_empty()) {
        None | Some("today" | "今天") => Ok(today),
        Some("tomorrow" | "明天") => Ok(today + Duration::days(1)),
        Some("后天") => Ok(today + Duration::days(2)),
        Some(text) => NaiveDate::parse_from_str(text, "%Y-%m-%d")
            .map_err(|_| format!("date 应为 today、tomorrow 或 YYYY-MM-DD，收到「{text}」")),
    }
}

fn period_arg(arguments: &Value, key: &str) -> Result<Option<u32>, String> {
    match arguments.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_u64()
            .filter(|period| (1..=LAST_PERIOD as u64).contains(period))
            .map(|period| Some(period as u32))
            .ok_or_else(|| format!("{key} 必须是 1 到 {LAST_PERIOD} 的节次")),
    }
}

pub(super) fn build_query(arguments: &Value, now: chrono::DateTime<chrono::FixedOffset>) -> Result<Query, String> {
    let today = now.date_naive();
    let date = parse_date(arguments, today)?;
    if date < today {
        return Err("只能查询今天及以后的空教室".into());
    }
    let is_today = date == today;
    let start = period_arg(arguments, "start_period")?;
    let end = period_arg(arguments, "end_period")?;
    let default_start = if is_today {
        match period_now(now.time()) {
            PeriodNow::InClass(n) | PeriodNow::Before(n) => n,
            PeriodNow::AfterAll => LAST_PERIOD + 1,
        }
    } else {
        1
    };
    let start = start.unwrap_or(default_start);
    let end = end.unwrap_or(LAST_PERIOD);
    if start > end && start <= LAST_PERIOD {
        return Err("start_period 不能大于 end_period".into());
    }
    Ok(Query {
        date,
        is_today,
        start,
        end,
        whole_range: arguments["end_period"].is_u64(),
        room: None,
    })
}

pub(super) async fn find_classroom(arguments: Value) -> Result<String, String> {
    let now = campus_time::now();
    let mut query = build_query(&arguments, now)?;
    let list = buildings().await?;
    let wanted = arguments["building"].as_str().map(str::trim).filter(|text| !text.is_empty());
    let form = |building: &Building| {
        vec![
            ("buildId", building.id.to_string()),
            ("courseDate", query.date.format("%Y-%m-%d").to_string()),
        ]
    };

    if let Some(wanted) = wanted {
        let campus_only = list.iter().any(|(campus, _)| campus.contains(wanted));
        if !campus_only {
            let (building, room) = match_building(&list, wanted).ok_or_else(|| {
                let names: Vec<String> = list.iter().map(|(campus, b)| format!("{}（{campus}）", b.name)).collect();
                format!("没有找到教学楼「{wanted}」，可查询：{}", names.join("、"))
            })?;
            query.room = room;
            let data: FloorData = post("/build/findBuildRoomType", &form(building)).await?;
            return Ok(clip(&render_building(&building.name, &data, &query)));
        }
    }

    // 没指定楼（或只说了校区）：各楼并发查询，汇总每栋楼的空教室数
    if query.start > query.end {
        return Ok("今天的上课时段已经结束（21:40 下课），教学楼可能已关闭，可以查明天的空教室。".into());
    }
    let targets: Vec<&(String, Building)> = list
        .iter()
        .filter(|(campus, _)| wanted.is_none_or(|wanted| campus.contains(wanted)))
        .collect();
    let form = &form;
    let results = join_all(targets.iter().map(|(campus, building)| async move {
        (campus, building, post::<FloorData>("/build/findBuildRoomType", &form(building)).await)
    }))
    .await;
    let mut text = if query.whole_range {
        format!(
            "{}月{}日 第 {}–{} 节全程空闲的教室数：\n",
            query.date.month(),
            query.date.day(),
            query.start,
            query.end
        )
    } else {
        format!("{}月{}日 从第 {} 节起空闲的教室数：\n", query.date.month(), query.date.day(), query.start)
    };
    let mut best: Option<(usize, String)> = None;
    for (campus, building, result) in &results {
        match result {
            Ok(data) => {
                let count = analyze(&building.name, data, &query).free.len();
                text.push_str(&format!("- {}（{campus}）：{count} 间\n", building.name));
                if best.as_ref().is_none_or(|(most, _)| count > *most) {
                    best = Some((count, building.name.clone()));
                }
            }
            Err(error) => text.push_str(&format!("- {}（{campus}）：查询失败（{error}）\n", building.name)),
        }
    }
    if let Some((count, name)) = best.filter(|(count, _)| *count > 0) {
        text.push_str(&format!("{name}空教室最多（{count} 间）。传 building 可看具体教室和空闲到几点。\n"));
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use serde_json::json;

    fn sample() -> FloorData {
        serde_json::from_value(json!({"floorList": [{
            "roomStuNumbs": [{"roomId": 1, "actualStuNum": 12}, {"roomId": 2, "actualStuNum": 0}],
            "children": [
                {"id": 1, "name": "东上院101", "roomCourseList": [
                    {"courseName": "高等数学", "teacherName": "张老师", "startSection": 1, "endSection": 2, "assignedWeeks": "1,2,3,4,5,6,7,8,9,10,11,12"},
                    {"courseName": "大学物理", "startSection": 9, "endSection": 10, "allWeeks": "#11#12#"}
                ]},
                {"id": 2, "name": "东上院102", "roomCourseList": [
                    {"courseName": "形势与政策", "startSection": 7, "endSection": 8, "assignedWeeks": "5,6,8,9"}
                ]},
                {"id": 3, "name": "东上院103", "roomCourseList": [
                    {"courseName": "线性代数", "startSection": 7, "endSection": 8, "assignedWeeks": ""}
                ]}
            ]
        }]}))
        .unwrap()
    }

    fn now(h: u32, m: u32) -> chrono::DateTime<chrono::FixedOffset> {
        campus_time::beijing().with_ymd_and_hms(2026, 11, 25, h, m, 0).unwrap()
    }

    #[test]
    fn rooms_free_now_are_sorted_by_how_long_they_stay_free() {
        // 第 11 周周三 14:10，第 7 节进行中
        let query = build_query(&json!({}), now(14, 10)).unwrap();
        assert_eq!((query.start, query.end, query.whole_range), (7, 14, false));
        let text = render_building("东上院", &sample(), &query);
        assert!(text.contains("东上院 · 11月25日 周三（第 11 周）"), "{text}");
        assert!(text.contains("从第 7 节起空闲的教室 2 间（共 3 间"), "{text}");
        // 102 的形势与政策第 11 周不上，之后都空；101 在第 9 节有课
        let first = text.find("- 东上院102：之后都没有课 · 实时 0 人").unwrap();
        let second = text.find("- 东上院101：空到第 8 节（16:00 起有课） · 实时 12 人").unwrap();
        assert!(first < second, "{text}");
        assert!(!text.contains("东上院103"), "103 每周第 7–8 节都有课：{text}");
    }

    #[test]
    fn explicit_ranges_tomorrow_and_single_room_views() {
        let query = build_query(&json!({"date": "tomorrow", "start_period": 1, "end_period": 2}), now(9, 0)).unwrap();
        assert!(query.whole_range && !query.is_today);
        let text = render_building("东上院", &sample(), &query);
        assert!(text.contains("第 1–2 节（08:00–09:40）全程空闲的教室 2 间"), "{text}");
        assert!(!text.contains("实时"), "非今天不显示实时人数：{text}");

        let mut query = build_query(&json!({}), now(9, 0)).unwrap();
        query.room = Some("东上院101".into());
        let text = render_building("东上院", &sample(), &query);
        assert!(text.contains("- 第 1–2 节（08:00–09:40）高等数学，张老师"), "{text}");
        assert!(text.contains("空闲时段：第 3–8 节（10:00–15:40）、第 11–14 节（18:00–21:40）"), "{text}");

        let late = build_query(&json!({}), now(22, 30)).unwrap();
        assert!(render_building("东上院", &sample(), &late).contains("上课时段已经结束"));
        assert!(build_query(&json!({"start_period": 9, "end_period": 3}), now(9, 0)).is_err());
        assert!(build_query(&json!({"start_period": 15}), now(9, 0)).is_err());
        assert!(build_query(&json!({"date": "2020-01-01"}), now(9, 0)).is_err());
    }

    #[test]
    fn building_names_use_longest_match_and_keep_room_numbers() {
        let list: Vec<(String, Building)> = ["上院", "东上院", "东中院", "陈瑞球楼", "新上院"]
            .iter()
            .enumerate()
            .map(|(id, name)| ("闵行校区".to_string(), Building { name: name.to_string(), id: id as u64 }))
            .collect();
        let name = |wanted| match_building(&list, wanted).map(|(b, room)| (b.name.clone(), room));
        assert_eq!(name("东上院"), Some(("东上院".into(), None)));
        assert_eq!(name("上院"), Some(("上院".into(), None)));
        assert_eq!(name("东上"), Some(("东上院".into(), None)));
        assert_eq!(name("东上院 102"), Some(("东上院".into(), Some("东上院102".into()))));
        assert_eq!(name("陈瑞球"), Some(("陈瑞球楼".into(), None)));
        assert_eq!(name("图书馆"), None);
    }
}
