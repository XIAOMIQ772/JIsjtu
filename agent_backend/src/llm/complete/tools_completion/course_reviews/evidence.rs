use std::collections::HashSet;
use std::fmt::Write;

use super::source::{Course, Review};
use crate::llm::complete::tools_completion::text::one_line;

struct Topic {
    aliases: &'static [&'static str],
    search: &'static [&'static str],
    related: &'static [&'static str],
}

const TOPICS: &[Topic] = &[
    Topic {
        aliases: &["attendance", "考勤", "签到", "点名", "出勤", "点到", "签退"],
        search: &[
            "签到",
            "点名",
            "考勤",
            "出勤",
            "签退",
            "点到",
            "抽查",
            "attendance",
        ],
        related: &[
            "点过名",
            "点过一次名",
            "点过两次名",
            "缺勤",
            "旷课",
            "查到课",
            "刷脸",
            "点卯",
            "roll call",
            "check-in",
        ],
    },
    Topic {
        aliases: &["grading", "grade", "给分", "评分", "成绩", "绩点"],
        search: &["给分", "成绩", "均分", "调分", "评分", "绩点"],
        related: &["压分", "捞人", "优秀率", "平时分", "grading", "grade"],
    },
    Topic {
        aliases: &["workload", "工作量", "负担", "作业", "考核", "考试"],
        search: &["作业", "工作量", "考试", "实验", "项目", "报告"],
        related: &[
            "期中", "期末", "测验", "耗时", "workload", "homework", "exam",
        ],
    },
    Topic {
        aliases: &["teaching", "授课", "讲课", "讲得", "教学", "讲授"],
        search: &["讲课", "授课", "教学", "讲解", "板书", "课堂"],
        related: &[
            "讲得", "讲授", "答疑", "互动", "节奏", "teaching", "lecture",
        ],
    },
];

const RISK_QUERIES: &[&str] = &["risk", "避雷", "踩雷", "劝退", "慎选"];
const RISK_PHRASES: &[&str] = &[
    "避雷",
    "踩雷",
    "慎选",
    "缺点",
    "不足",
    "不推荐",
    "不建议",
    "扣分",
    "最大的问题",
];

fn terms(focus: &str, extra: bool) -> Vec<&str> {
    let query = focus.trim().to_ascii_lowercase();
    let mut words = Vec::new();
    for topic in TOPICS
        .iter()
        .filter(|t| t.aliases.iter().any(|alias| query.contains(alias)))
    {
        words.extend_from_slice(topic.search);
        if extra {
            words.extend_from_slice(topic.related);
        }
    }
    if !words.is_empty() {
        return words;
    }
    if ["overall", "general", "综合", "推荐", "评价", "口碑"]
        .iter()
        .chain(RISK_QUERIES)
        .any(|word| query.contains(word))
    {
        return Vec::new();
    }
    focus
        .split(|c: char| c.is_whitespace() || "、，,/|".contains(c))
        .filter(|s| !s.is_empty())
        .take(6)
        .collect()
}

/// Query words for a single OR search. General comparisons also need reviews without topic keywords.
pub(super) fn search_terms(focus: &str) -> Vec<&str> {
    terms(focus, false)
}

fn match_range(text: &str, words: &[&str]) -> Option<(usize, usize)> {
    if words.is_empty() {
        return None;
    }
    let text = text.to_ascii_lowercase();
    let mut first = usize::MAX;
    let mut last = 0;
    for word in words {
        let word = word.to_ascii_lowercase();
        if let Some(at) = text.find(&word) {
            first = first.min(at);
            last = last.max(text.rfind(&word).unwrap_or(at) + word.len());
        }
    }
    (first != usize::MAX).then_some((first, last))
}

fn excerpt(text: &str, words: &[&str]) -> String {
    const WIDTH: usize = 156;
    let text = one_line(text);
    let chars: Vec<_> = text.chars().collect();
    if chars.len() <= WIDTH {
        return text;
    }
    let window = |start: usize, end: usize| chars[start..end].iter().collect::<String>();
    let Some((first, last)) = match_range(&text, words) else {
        return format!(
            "{}…{}",
            window(0, WIDTH / 2),
            window(chars.len() - WIDTH / 2, chars.len())
        );
    };
    let (first, last) = (text[..first].chars().count(), text[..last].chars().count());
    // Keep qualifiers on both sides of a mention. Widely separated mentions can describe different weeks.
    let (start, end, body) = if last - first > WIDTH * 2 / 3 {
        let left = first.saturating_sub(WIDTH / 4);
        let right = last.saturating_sub(WIDTH / 4).min(chars.len() - WIDTH / 2);
        if left + WIDTH / 2 < right {
            (
                left,
                right + WIDTH / 2,
                format!(
                    "{}…{}",
                    window(left, left + WIDTH / 2),
                    window(right, right + WIDTH / 2)
                ),
            )
        } else {
            let start = left.min(chars.len() - WIDTH);
            (start, start + WIDTH, window(start, start + WIDTH))
        }
    } else {
        let margin = (WIDTH - (last - first)) / 2;
        let start = first.saturating_sub(margin).min(chars.len() - WIDTH);
        (start, start + WIDTH, window(start, start + WIDTH))
    };
    format!(
        "{}{}{}",
        if start > 0 { "…" } else { "" },
        body,
        if end < chars.len() { "…" } else { "" }
    )
}

/// Select excerpts, including distinct attendance accounts when available. The wording stays with the reviewer.
fn sample<'a>(mut candidates: Vec<&'a Review>, limit: usize, attendance: bool) -> Vec<&'a Review> {
    candidates.sort_by(|a, b| {
        (&b.semester, &b.created_at, b.id).cmp(&(&a.semester, &a.created_at, a.id))
    });
    if candidates.len() <= limit {
        return candidates;
    }
    let rated = || {
        candidates
            .iter()
            .copied()
            .filter(|r| (1..=5).contains(&r.rating))
    };
    let first = candidates.first().copied();
    let mention = |phrases: &[&str]| {
        attendance
            .then(|| {
                candidates
                    .iter()
                    .copied()
                    .find(|r| phrases.iter().any(|p| r.content.contains(p)))
            })
            .flatten()
    };
    let no_roll_call = mention(&[
        "不点名",
        "没点过名",
        "没有点名",
        "不签到",
        "没有签到",
        "无考勤",
        "不考勤",
    ]);
    let roll_call = mention(&[
        "偶尔点名",
        "随机点名",
        "点过名",
        "点过一次名",
        "点过两次名",
        "偶尔签到",
        "每节课签到",
        "需要签到",
        "签到计入",
        "抽查",
    ]);
    let lowest = rated().min_by_key(|r| r.rating);
    let highest = rated().min_by_key(|r| std::cmp::Reverse(r.rating));
    let mut semesters = HashSet::new();
    let mut ids = HashSet::new();
    let distinct_terms = candidates
        .iter()
        .copied()
        .filter(|r| !r.semester.is_empty() && semesters.insert(&r.semester));
    [first, no_roll_call, roll_call, lowest, highest]
        .into_iter()
        .flatten()
        .chain(distinct_terms)
        .chain(candidates.iter().copied())
        .filter(|r| ids.insert(r.id))
        .take(limit)
        .collect()
}

pub(super) fn course_evidence(
    course: &Course,
    reviews: &[Review],
    focus: &str,
    max_reviews: usize,
) -> String {
    let mut teachers = Vec::new();
    for teacher in std::iter::once(&course.main_teacher).chain(&course.teacher_group) {
        let name = one_line(&teacher.name);
        if !name.is_empty() && !teachers.contains(&name) {
            teachers.push(name);
        }
    }
    let instructors = match teachers.len() {
        0 => "主讲教师未列出".to_string(),
        1 => format!("主讲 {}", teachers[0]),
        _ => format!("共授 {}（条目评分涵盖教师组）", teachers.join("、")),
    };
    let mut out = format!(
        "{}（{}） · {instructors} · [课程页]({})\n",
        one_line(&course.name),
        one_line(&course.code),
        course.url()
    );
    let focus = focus.to_ascii_lowercase();
    let risk_query = RISK_QUERIES.iter().any(|word| focus.contains(word));
    if course.rating.count == 0 {
        out.push_str("暂无站内评分（0 条）");
    } else if (1.0..=5.0).contains(&course.rating.avg) {
        let _ = write!(
            out,
            "课程条目均分 {:.2}/5，{} 条评分",
            course.rating.avg, course.rating.count
        );
        let counts = &course.rating.distribution;
        if risk_query && counts.iter().sum::<usize>() == course.rating.count {
            let _ = write!(out, "（其中 1–2 星 {} 条）", counts[0] + counts[1]);
        }
    } else {
        let _ = write!(out, "站内有 {} 条评分，均分未提供", course.rating.count);
    }
    if !course.department.is_empty() {
        let _ = write!(out, " · {}", one_line(&course.department));
    }
    if !course.last_semester.is_empty() {
        let _ = write!(out, " · 最近开课 {}", one_line(&course.last_semester));
    }
    let mut unique = HashSet::new();
    let texts: Vec<_> = reviews
        .iter()
        .filter(|r| r.course_id == course.id && !r.content.trim().is_empty() && unique.insert(r.id))
        .collect();
    if texts.is_empty() {
        out.push_str("\n本次未读到有正文的学生点评。");
        return out;
    }
    let words = terms(&focus, true);
    let read_count = texts.len();
    let matches: Vec<_> = texts
        .into_iter()
        .filter(|r| words.is_empty() || match_range(&r.content, &words).is_some())
        .collect();
    if matches.is_empty() {
        let _ = write!(
            out,
            "\n本次读到的 {read_count} 条文字点评中未检索到与关注点有关的表述，该项情况未知。"
        );
        return out;
    }
    let chosen = sample(matches, max_reviews, words.contains(&"签到"));
    let _ = write!(
        out,
        "\n本次读到 {read_count} 条文字点评，摘录 {} 条。",
        chosen.len()
    );
    let excerpt_words = if words.is_empty() && risk_query {
        RISK_PHRASES
    } else {
        &words
    };
    for review in chosen {
        let term = if !review.semester.is_empty() {
            one_line(&review.semester)
        } else if let Some((date, _)) = review.created_at.split_once('T') {
            format!("学期未标，{} 发布", one_line(date))
        } else {
            "学期未标".into()
        };
        let stars = if (1..=5).contains(&review.rating) {
            format!("{}/5 星", review.rating)
        } else {
            "星级未标".into()
        };
        let _ = write!(
            out,
            "\n- {term} · {stars} · “{}” [点评原文]({})",
            excerpt(&review.content, excerpt_words),
            review.url()
        );
    }
    out
}
