mod auth;
mod evidence;
mod source;

use std::collections::{BTreeMap, HashSet};
use std::fmt::Write;
use std::future::Future;

use serde::Deserialize;

use self::auth::CourseReviewClient;
use self::source::{Course, Page, ReviewOrder, Teacher};
use super::text::one_line;

const CATALOG_SIZE: usize = 80;
const COURSES_PER_PAGE: usize = 4;
const CHOICES_PER_PAGE: usize = 12;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Query {
    course: Option<String>,
    teacher: Option<String>,
    course_id: Option<u64>,
    teacher_id: Option<u64>,
    focus: Option<String>,
    page: Option<usize>,
}

fn key(text: &str) -> String {
    text.split_whitespace().collect::<String>().to_lowercase()
}

fn page_of<T>(rows: &[T], number: usize, size: usize) -> Result<(&[T], String), String> {
    let pages = rows.len().div_ceil(size);
    if number == 0 || number > pages {
        return Err(format!("当前结果共 {pages} 页，请使用范围内的页码。"));
    }
    let start = (number - 1) * size;
    let mut label = format!("共 {} 项，第 {number}/{pages} 页。", rows.len());
    if number < pages {
        let _ = write!(label, "用相同条件和 page={} 可继续。", number + 1);
    }
    Ok((&rows[start..(start + size).min(rows.len())], label))
}

/// Bound catalog reads for broad free-text searches. Review bodies are only fetched for the displayed courses.
async fn catalog<T, F, Fut>(fetch: F) -> Result<(Vec<T>, String), String>
where
    F: Fn(usize) -> Fut,
    Fut: Future<Output = Result<Page<T>, String>>,
{
    let first = fetch(1).await?;
    let total = first.total;
    let mut rows = first.items;
    if rows.len() < total && rows.len() == CATALOG_SIZE {
        rows.extend(fetch(2).await?.items);
    }
    let note = if rows.len() < total {
        format!(
            "站内命中 {total} 项，已读取目录前 {} 项。可以用更完整的名称或课号缩小范围。\n",
            rows.len()
        )
    } else {
        String::new()
    };
    Ok((rows, note))
}

enum TeacherMatch {
    One(Teacher),
    Choices(String),
}

async fn find_teacher(
    client: &CourseReviewClient,
    name: &str,
    page: usize,
) -> Result<TeacherMatch, String> {
    let (mut rows, note) = catalog(|p| source::teachers(client, name, p, CATALOG_SIZE)).await?;
    if rows.is_empty() {
        return Ok(TeacherMatch::Choices(format!(
            "选课社区没有搜到教师“{}”。",
            one_line(name)
        )));
    }
    let needle = key(name);
    let exact = rows.iter().any(|t| key(&t.name) == needle);
    if exact {
        rows.retain(|t| key(&t.name) == needle);
    }
    if rows.len() == 1 && note.is_empty() {
        return Ok(TeacherMatch::One(rows.remove(0)));
    }
    let (shown, label) = page_of(&rows, page, CHOICES_PER_PAGE)?;
    let mut out = format!("{note}请选择教师，确认后传 teacher_id。{label}");
    for t in shown {
        let _ = write!(
            out,
            "\n- {} · {} {} · teacher_id={} · {}",
            one_line(&t.name),
            one_line(&t.department),
            one_line(&t.title),
            t.id,
            t.url()
        );
    }
    Ok(TeacherMatch::Choices(out))
}

fn course_choices(courses: &[Course], page: usize, note: &str) -> Result<String, String> {
    let mut families = BTreeMap::new();
    for c in courses {
        *families.entry((&c.name, &c.code)).or_insert(0_usize) += 1;
    }
    let rows: Vec<_> = families.into_iter().collect();
    let (shown, label) = page_of(&rows, page, CHOICES_PER_PAGE)?;
    let mut out = format!("{note}找到了多门课程，请按名称或课号重查以比较授课教师。{label}");
    for ((name, code), count) in shown {
        let _ = write!(
            out,
            "\n- {}（{}）· {count} 条授课记录",
            one_line(name),
            one_line(code)
        );
    }
    Ok(out)
}

async fn review_evidence(
    client: &CourseReviewClient,
    course: &Course,
    focus: &str,
    limit: usize,
) -> String {
    let fetch = |keyword, rating, order, size| {
        source::reviews(client, course.id, keyword, rating, order, 1, size)
    };
    let recent = fetch(None, None, ReviewOrder::Recent, 10).await;
    let mut notes = Vec::new();
    let (mut reviews, total) = match recent {
        Ok(page) => (page.items, page.total),
        Err(err) => {
            let out = evidence::course_evidence(course, &[], focus, limit);
            return format!("{out}\n点评正文读取失败：{err}");
        }
    };
    if reviews.len() < total {
        let query = evidence::search_terms(focus).join(" OR ");
        let mut selections = Vec::new();
        if !query.is_empty() {
            selections.push(("主题检索", Some(query.as_str()), None, ReviewOrder::Recent));
        }
        selections.push(("高赞点评", None, None, ReviewOrder::Popular));
        if matches!(focus, "risks" | "避雷" | "overall" | "综合") {
            if let Some(i) = course.rating.distribution[..2]
                .iter()
                .position(|count| *count > 0)
            {
                selections.push(("低星点评", None, Some((i + 1) as u8), ReviewOrder::Recent));
            }
        }
        for (label, query, stars, order) in selections {
            match fetch(query, stars, order, 6).await {
                Ok(page) => reviews.extend(page.items),
                Err(err) => {
                    notes.push(format!("{label}未能补全：{err}"));
                    break;
                }
            }
        }
    }
    let mut ids = HashSet::new();
    reviews.retain(|r| ids.insert(r.id));
    let mut out = evidence::course_evidence(course, &reviews, focus, limit);
    if reviews.len() < total {
        let _ = write!(out, "\n站内共 {total} 条点评，本次为抽样。");
    }
    for note in notes {
        let _ = write!(out, "\n{note}");
    }
    out
}

async fn read_record(
    client: &CourseReviewClient,
    c: &Course,
    focus: &str,
    limit: usize,
    member: Option<u64>,
) -> Option<String> {
    match source::course(client, c.id).await {
        Ok(detail) => {
            if member.is_some_and(|id| {
                detail.main_teacher.id != id && !detail.teacher_group.iter().any(|t| t.id == id)
            }) {
                return None;
            }
            Some(review_evidence(client, &detail, focus, limit).await)
        }
        Err(err) => Some(format!(
            "{}（{}）· {} · {}\n授课详情读取失败：{err}",
            one_line(&c.name),
            one_line(&c.code),
            one_line(&c.main_teacher.name),
            c.url()
        )),
    }
}

pub(super) async fn course_reviews(args: serde_json::Value) -> Result<String, String> {
    let q: Query = serde_json::from_value(args)
        .map_err(|_| "选课评价参数有误，请使用课程名称、课号、教师姓名或站内 ID。")?;
    let course_name = q.course.as_deref().unwrap_or_default().trim();
    let teacher_name = q.teacher.as_deref().unwrap_or_default().trim();
    let focus = q
        .focus
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("overall");
    let page = q.page.unwrap_or(1);
    if page == 0 || q.teacher_id == Some(0) || q.course_id == Some(0) {
        return Err("页码、course_id 和 teacher_id 都从 1 开始。".into());
    }
    if [course_name, teacher_name, focus]
        .iter()
        .any(|s| s.chars().count() > 120)
    {
        return Err("课程、教师和关注点请各控制在 120 字以内。".into());
    }
    if course_name.is_empty()
        && teacher_name.is_empty()
        && q.course_id.is_none()
        && q.teacher_id.is_none()
    {
        return Err("请提供课程名称、课号、教师姓名或选课社区的课程/教师 ID。".into());
    }
    let client = CourseReviewClient::new()?;
    if let Some(id) = q.course_id {
        if page != 1 {
            return Err("course_id 对应单条授课记录，请用第 1 页。".into());
        }
        let detail = source::course(&client, id).await?;
        let matches_teacher = std::iter::once(&detail.main_teacher)
            .chain(&detail.teacher_group)
            .any(|t| {
                q.teacher_id.is_none_or(|id| t.id == id)
                    && (teacher_name.is_empty() || key(&t.name) == key(teacher_name))
            });
        if !matches_teacher
            || (!course_name.is_empty()
                && key(&detail.code) != key(course_name)
                && key(&detail.name) != key(course_name))
        {
            return Err(
                "此 course_id 的课程或授课教师与给定条件不符，请核对选课社区的记录。".into(),
            );
        }
        return Ok(review_evidence(&client, &detail, focus, 7).await);
    }
    let teacher = match q.teacher_id {
        Some(id) => {
            let t = source::teacher(&client, id).await?;
            if !teacher_name.is_empty() && key(&t.name) != key(teacher_name) {
                return Err("此 teacher_id 与给定姓名不符，请核对选课社区的教师记录。".into());
            }
            Some(t)
        }
        None if !teacher_name.is_empty() => {
            match find_teacher(&client, teacher_name, page).await? {
                TeacherMatch::One(t) => Some(t),
                TeacherMatch::Choices(text) => return Ok(text),
            }
        }
        None => None,
    };
    let teacher_id = teacher.as_ref().map(|t| t.id);
    // The site's teacher index lists primary instructors. For a named course, check its full teaching team.
    let (teacher_index, member) = if course_name.is_empty() {
        (teacher_id, None)
    } else {
        (None, teacher_id)
    };
    let (mut courses, note) =
        catalog(|p| source::courses(&client, course_name, teacher_index, p, CATALOG_SIZE)).await?;
    let teacher_label = teacher.as_ref().map_or(String::new(), |t| {
        format!(
            "教师 {} · {} · {}\n",
            one_line(&t.name),
            one_line(&t.department),
            t.url()
        )
    });
    let scope = if member.is_some() {
        "逐项核对主讲及合授教师。候选授课记录"
    } else if teacher_index.is_some() {
        "社区归在该教师名下的主讲课程按点评数排列。"
    } else {
        "选课社区授课记录按点评数排列。"
    };
    if courses.is_empty() {
        return Ok(format!("{teacher_label}选课社区未找到符合条件的授课记录。"));
    }
    let mut code_note = "";
    if !course_name.is_empty() {
        let needle = key(course_name);
        let exact = |c: &Course| key(&c.name) == needle || key(&c.code) == needle;
        if courses.iter().any(exact) {
            courses.retain(exact);
        } else if courses
            .iter()
            .any(|c| c.name != courses[0].name || c.code != courses[0].code)
        {
            return course_choices(&courses, page, &format!("{teacher_label}{note}"));
        }
        if courses.iter().any(|c| c.code != courses[0].code) {
            code_note = "同一课程名称有多个课号，请按课号分别比较，或用具体课号缩小范围。\n";
        }
    }
    courses.sort_by_key(|c| {
        (
            member.is_some_and(|id| c.main_teacher.id != id),
            std::cmp::Reverse(c.rating.count),
            c.id,
        )
    });
    let (shown, label) = page_of(&courses, page, COURSES_PER_PAGE)?;
    let limit = if shown.len() == 1 { 7 } else { 4 };
    let mut details = Vec::new();
    for pair in shown.chunks(2) {
        let first = read_record(&client, &pair[0], focus, limit, member);
        if let Some(other) = pair.get(1) {
            let (a, b) = tokio::join!(first, read_record(&client, other, focus, limit, member));
            details.extend(a);
            details.extend(b);
        } else {
            details.extend(first.await);
        }
    }
    if details.is_empty() {
        details.push("本页候选记录中没有该教师。".into());
    }
    Ok(format!(
        "{teacher_label}{note}{code_note}{scope}{label}\n\n{}",
        details.join("\n\n")
    ))
}
