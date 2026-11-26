use std::sync::{Arc, LazyLock};

use regex::Regex;

use super::MAX_OUTPUT_CHARS;

static KEYWORD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(?:problem|question|exercise|task|hw|q|习题|题目|练习)\s*[#.:：]?\s*(\d{1,3}(?:\.\d{1,3})?)(?:\D|$)").unwrap()
});
static CHINESE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^第\s*[一二三四五六七八九十百零〇两\d]{1,4}\s*[题问]").unwrap());
static SECTION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(?:[一二三四五六七八九十百]{1,4}\s*[、.．]|第\s*[一二三四五六七八九十百\d]+\s*部分|(?:part|section)\s+(?:[a-z]|[ivxlcdm]+|\d+)(?:\s|[:：.．、–—-]|$))").unwrap()
});
static DOTTED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^((?:\d{1,2}\.){1,3})(\d{1,3})\.?(?:\s|$)").unwrap());
static ARABIC: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(\d{1,3})\s*[.．、)）](\S?)").unwrap());
static PAREN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[(（]\s*(\d{1,3})\s*[)）]").unwrap());
static SUBPART: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^(?:[(（]\s*([a-z]|[ivx]{1,5})\s*[)）]|([a-z])[.)]\s)").unwrap());
static POINTS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)[(（\[【]\s*(\d+(?:\.\d+)?)\s*(?:points?|pts?\.?|marks?|分)\s*[)）\]】]").unwrap()
});

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum QuestionStyle {
    Keyword,
    ChineseTi,
    Dotted,
    Arabic,
    Paren,
    Section,
    Whole,
    Mixed,
}

impl QuestionStyle {
    fn describe(self) -> &'static str {
        match self {
            Self::Keyword => "Problem/Question/习题 N",
            Self::ChineseTi => "第N题/问",
            Self::Dotted => "2.1.1 / 3.4",
            Self::Arabic => "1. / 1) / 1、",
            Self::Paren => "(1)",
            Self::Section => "按章节",
            Self::Whole => "未识别到编号，整体视为一题",
            Self::Mixed => "多种编号",
        }
    }
}

struct QuestionHeader<'a> {
    line: usize,
    indent: usize,
    style: QuestionStyle,
    label: &'a str,
    prefix: &'a str,
    number: u32,
}

pub(super) struct Question {
    pub(super) label: String,
    pub(super) section: Option<Arc<str>>,
    section_intro: Arc<str>,
    source_intro: Arc<str>,
    pub(super) first_page: usize,
    pub(super) last_page: usize,
    pub(super) points: Option<String>,
    pub(super) subparts: Vec<String>,
    pub(super) text: String,
}

pub(super) struct ParsedHomework {
    pub(super) style: QuestionStyle,
    pub(super) preamble: String,
    pub(super) questions: Vec<Question>,
}

impl ParsedHomework {
    fn empty() -> Self {
        Self { style: QuestionStyle::Whole, preamble: String::new(), questions: Vec::new() }
    }

    fn add_style(&mut self, style: QuestionStyle) {
        if self.questions.is_empty() {
            self.style = style;
        } else if self.style != style {
            self.style = QuestionStyle::Mixed;
        }
    }
}

fn question_header(line: usize, raw: &str) -> Option<QuestionHeader<'_>> {
    let text = raw.trim_start();
    let indent = raw[..raw.len() - text.len()].chars().map(|c| if c == '\t' { 4 } else { 1 }).sum();
    let header =
        |style, label, prefix, number| Some(QuestionHeader { line, indent, style, label, prefix, number });
    if let Some(caps) = KEYWORD.captures(text) {
        return header(QuestionStyle::Keyword, &text[..caps.get(1)?.end()], "", 0);
    }
    if let Some(found) = CHINESE.find(text) {
        return header(QuestionStyle::ChineseTi, found.as_str(), "", 0);
    }
    if let Some(caps) = PAREN.captures(text) {
        return header(QuestionStyle::Paren, caps.get(0)?.as_str(), "", caps[1].parse().ok()?);
    }
    if let Some(caps) = DOTTED.captures(text) {
        return header(
            QuestionStyle::Dotted,
            &text[..caps.get(2)?.end()],
            caps.get(1)?.as_str(),
            caps[2].parse().ok()?,
        );
    }
    let caps = ARABIC.captures(text)?;
    // 紧跟小数点的数字不是题干，如 “3.5”。
    if caps[2].starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    header(QuestionStyle::Arabic, caps.get(1)?.as_str(), "", caps[1].parse().ok()?)
}

/// 教材选题可以跳号。等长时选较早的编号，以免把从 1 重新编号的内部步骤当作新题。
fn numbered_chain<'a>(candidates: &[&'a QuestionHeader<'a>]) -> Vec<&'a QuestionHeader<'a>> {
    if candidates.is_empty() {
        return Vec::new();
    }
    let mut lengths = vec![1; candidates.len()];
    let mut next = vec![None; candidates.len()];
    let mut start = candidates.len() - 1;
    for (index, current) in candidates.iter().enumerate().rev() {
        for (later, other) in candidates.iter().enumerate().skip(index + 1) {
            let increases = if other.prefix == current.prefix {
                other.number > current.number
            } else {
                other.number == 1
            };
            if increases && lengths[later] + 1 > lengths[index] {
                lengths[index] = lengths[later] + 1;
                next[index] = Some(later);
            }
        }
        if lengths[index] >= lengths[start] {
            start = index;
        }
    }
    std::iter::successors(Some(start), |index| next[*index]).map(|index| candidates[index]).collect()
}

fn section_headers<'a>(headers: &'a [QuestionHeader<'a>]) -> Vec<&'a QuestionHeader<'a>> {
    let mut explicit: Vec<_> = headers
        .iter()
        .filter(|h| matches!(h.style, QuestionStyle::Keyword | QuestionStyle::ChineseTi))
        .collect();
    if !explicit.is_empty() {
        // 明确的大题下，“第…问”是子问。
        if explicit.iter().any(|h| h.style == QuestionStyle::Keyword || h.label.ends_with('题')) {
            explicit.retain(|h| !h.label.ends_with('问'));
        }
        explicit.dedup_by(|later, earlier| later.label.eq_ignore_ascii_case(earlier.label));
        return explicit;
    }
    let deepest = headers.iter().map(|h| h.prefix.matches('.').count()).max().unwrap_or(0);
    let mut best = Vec::new();
    for style in [QuestionStyle::Dotted, QuestionStyle::Arabic, QuestionStyle::Paren] {
        let mut candidates: Vec<_> = headers
            .iter()
            .filter(|h| {
                h.style == style
                    && (style != QuestionStyle::Dotted || h.prefix.matches('.').count() == deepest)
            })
            .collect();
        if let Some(outer) = candidates.iter().map(|h| h.indent).min() {
            // 至少两个外层编号确定了层级时，更深的编号留作题内步骤。
            if candidates.iter().filter(|h| h.indent <= outer + 1).count() >= 2 {
                candidates.retain(|h| h.indent <= outer + 1);
            }
        }
        let chain = numbered_chain(&candidates);
        if chain.len() > best.len() {
            best = chain;
        }
    }
    best
}

type Line<'a> = (usize, &'a str);

fn source_text(lines: &[Line<'_>]) -> String {
    lines.iter().map(|(_, raw)| *raw).collect::<Vec<_>>().join("\n")
}

fn make_question(
    lines: &[Line<'_>],
    label: &str,
    style: QuestionStyle,
    section: &Option<Arc<str>>,
    section_intro: &Arc<str>,
) -> Question {
    let text = source_text(lines);
    let head: String = text.chars().take(300).collect();
    let points = POINTS.captures(&head).map(|caps| caps[1].to_string());
    let mut subparts = Vec::new();
    for (_, raw) in &lines[1..] {
        let line = raw.trim_start();
        let label = SUBPART
            .captures(line)
            .map(|caps| format!("({})", caps.get(1).or(caps.get(2)).unwrap().as_str()))
            .or_else(|| {
                (style != QuestionStyle::Paren)
                    .then(|| PAREN.captures(line).map(|c| format!("({})", &c[1])))
                    .flatten()
            })
            .or_else(|| {
                CHINESE.find(line).filter(|m| m.as_str().ends_with('问')).map(|m| m.as_str().to_owned())
            });
        if let Some(label) = label {
            if !subparts.contains(&label) {
                subparts.push(label);
            }
        }
    }
    let last_page = lines.iter().rev().find(|(_, raw)| !raw.trim().is_empty()).unwrap_or(&lines[0]).0;
    Question {
        label: label.to_owned(),
        section: section.clone(),
        section_intro: section_intro.clone(),
        source_intro: Arc::default(),
        first_page: lines[0].0,
        last_page,
        points,
        subparts,
        text,
    }
}

pub(super) fn split_questions(pages: &[String]) -> ParsedHomework {
    let lines: Vec<_> = pages
        .iter()
        .enumerate()
        .flat_map(|(page, text)| text.lines().map(move |raw| (page + 1, raw)))
        .collect();
    let mut parsed = ParsedHomework::empty();
    let mut boundaries = vec![0];
    boundaries.extend((1..lines.len()).filter(|&index| SECTION.is_match(lines[index].1.trim_start())));
    boundaries.push(lines.len());
    for range in boundaries.windows(2) {
        let group = &lines[range[0]..range[1]];
        if !group.iter().any(|(_, raw)| !raw.trim().is_empty()) {
            continue;
        }
        let section = SECTION.find(group[0].1.trim_start());
        let headers: Vec<_> = group
            .iter()
            .enumerate()
            .skip(usize::from(section.is_some()))
            .filter_map(|(line, (_, raw))| question_header(line, raw))
            .collect();
        let starts = section_headers(&headers);
        if starts.is_empty() {
            if section.is_none() && range[1] < lines.len() {
                parsed.preamble = source_text(group);
                continue;
            }
            let style = if section.is_some() { QuestionStyle::Section } else { QuestionStyle::Whole };
            let label = section.map(|m| m.as_str().trim()).unwrap_or("全文");
            parsed.add_style(style);
            parsed.questions.push(make_question(group, label, style, &None, &Arc::default()));
            continue;
        }
        let intro = source_text(&group[..starts[0].line]);
        let (section, section_intro) = if section.is_some() {
            (Some(Arc::from(group[0].1.trim())), Arc::from(intro))
        } else {
            parsed.preamble = intro;
            (None, Arc::default())
        };
        for (index, start) in starts.iter().enumerate() {
            let end = starts.get(index + 1).map_or(group.len(), |h| h.line);
            parsed.add_style(start.style);
            parsed.questions.push(make_question(
                &group[start.line..end],
                start.label,
                start.style,
                &section,
                &section_intro,
            ));
        }
    }
    parsed
}

pub(super) fn split_question_sources(sources: &[Vec<String>]) -> ParsedHomework {
    if let [pages] = sources {
        return split_questions(pages);
    }
    let mut combined = ParsedHomework::empty();
    let mut page_offset = 0;
    for pages in sources {
        let parsed = split_questions(pages);
        if !parsed.questions.is_empty() {
            combined.add_style(parsed.style);
        }
        let intro: Arc<str> = parsed.preamble.into();
        for mut question in parsed.questions {
            question.source_intro = intro.clone();
            question.first_page += page_offset;
            question.last_page += page_offset;
            combined.questions.push(question);
        }
        page_offset += pages.len();
    }
    combined
}

fn question_title(index: usize, q: &Question) -> String {
    let mut title = format!("第 {} 题 · {}", index + 1, q.label);
    if let Some(section) = &q.section {
        let short: String = section.chars().take(60).collect();
        title.push_str(&format!(" · 所属：{short}"));
    }
    if q.first_page == q.last_page {
        title.push_str(&format!(" · 第 {} 页", q.first_page));
    } else {
        title.push_str(&format!(" · 第 {}–{} 页", q.first_page, q.last_page));
    }
    if let Some(points) = &q.points {
        title.push_str(&format!(" · {points} 分"));
    }
    if !q.subparts.is_empty() {
        title.push_str(&format!(" · 小问 {}", q.subparts.join("")));
    }
    title
}

pub(super) fn part_number(args: &serde_json::Value) -> Result<usize, String> {
    let Some(value) = args.get("part").filter(|v| !v.is_null()) else {
        return Ok(1);
    };
    value
        .as_u64()
        .or_else(|| value.as_str()?.trim().parse().ok())
        .and_then(|number| usize::try_from(number).ok())
        .filter(|number| *number > 0)
        .ok_or_else(|| "part 必须是从 1 开始的整数。".to_owned())
}

/// 给续读提示预留空间，分段边界保留换行及 UTF-8 字符的所有字节。
pub(super) fn homework_page(text: &str, part: usize) -> Result<String, String> {
    const BODY_BYTES: usize = MAX_OUTPUT_CHARS - 256;
    let mut chunks = Vec::new();
    let mut rest = text;
    while rest.len() > BODY_BYTES {
        let max = rest.floor_char_boundary(BODY_BYTES);
        let split = rest[..max].rfind('\n').filter(|&n| n >= max / 2).map_or(max, |n| n + 1);
        let (chunk, tail) = rest.split_at(split);
        chunks.push(chunk);
        rest = tail;
    }
    chunks.push(rest);
    let Some(&chunk) = part.checked_sub(1).and_then(|i| chunks.get(i)) else {
        return Err(format!("part 超出范围，共 {} 段，请传 1–{} 的段号", chunks.len(), chunks.len()));
    };
    if chunks.len() == 1 {
        return Ok(chunk.to_owned());
    }
    let next = if part < chunks.len() {
        format!("，保持其他参数，传 part={} 继续读取", part + 1)
    } else {
        "，已到末段".to_owned()
    };
    Ok(format!("[第 {part}/{} 段{next}]\n{chunk}", chunks.len()))
}

fn add_intro(text: &mut String, label: &str, intro: &str) {
    if !intro.trim().is_empty() {
        text.push_str(&format!("\n[{label}]\n{intro}\n"));
    }
}

fn question_index(questions: &[Question], wanted: &str) -> Result<usize, String> {
    let wanted = wanted.trim();
    let count = questions.len();
    if let Some(index) = wanted.parse::<usize>().ok().and_then(|n| n.checked_sub(1)).filter(|&i| i < count) {
        return Ok(index);
    }
    let matches: Vec<_> = questions
        .iter()
        .enumerate()
        .filter(|(_, q)| q.label.eq_ignore_ascii_case(wanted))
        .map(|(i, _)| i)
        .collect();
    match matches.as_slice() {
        [index] => Ok(*index),
        [] => Err(format!("没有编号为「{wanted}」的题目，共 {count} 题，可传 1–{count} 的序号")),
        _ => {
            let numbers: Vec<_> = matches.iter().map(|i| (i + 1).to_string()).collect();
            Err(format!("「{wanted}」对应多道题，请从序号 {} 中选择一道。", numbers.join("、")))
        }
    }
}

pub(super) fn render_questions(
    header: &str,
    parsed: &ParsedHomework,
    only: Option<&str>,
    part: usize,
) -> Result<String, String> {
    let count = parsed.questions.len();
    if count == 0 {
        return Err(format!("{header}\n没有读到可识别的题目文字"));
    }
    if let Some(wanted) = only {
        let index = question_index(&parsed.questions, wanted)?;
        let q = &parsed.questions[index];
        let mut answer = format!("{header}\n=== {} ===\n", question_title(index, q));
        add_intro(&mut answer, "题前说明", &parsed.preamble);
        add_intro(&mut answer, "附件题前说明", &q.source_intro);
        add_intro(&mut answer, "本节说明", &q.section_intro);
        answer.push_str(&q.text);
        return homework_page(&answer, part);
    }

    let heading = format!("识别到 {count} 道题（编号样式：{}）\n{header}", parsed.style.describe());
    let mut full = heading.clone();
    add_intro(&mut full, "题前说明", &parsed.preamble);
    let mut last_source = "";
    let mut last_section = "";
    for (index, q) in parsed.questions.iter().enumerate() {
        if q.source_intro.as_ref() != last_source {
            add_intro(&mut full, "附件题前说明", &q.source_intro);
            last_source = &q.source_intro;
            last_section = "";
        }
        if q.section_intro.as_ref() != last_section {
            add_intro(&mut full, "本节说明", &q.section_intro);
            last_section = &q.section_intro;
        }
        full.push_str(&format!("\n=== {} ===\n{}\n", question_title(index, q), q.text));
        if full.len() >= MAX_OUTPUT_CHARS {
            let mut outline = format!(
                "{heading}\n内容较长。用 question=序号 逐题读取题干和适用说明；长题会给出下一段的 part。\n"
            );
            for (index, question) in parsed.questions.iter().enumerate() {
                let start: String = question.text.chars().take(100).collect();
                let excerpt = start.split_whitespace().collect::<Vec<_>>().join(" ");
                outline.push_str(&format!("\n{}\n  {excerpt}\n", question_title(index, question)));
            }
            return homework_page(&outline, part);
        }
    }
    homework_page(&full, part)
}
