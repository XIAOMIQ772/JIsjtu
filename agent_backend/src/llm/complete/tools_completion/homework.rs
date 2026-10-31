use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex, OnceLock};

use regex::Regex;

use super::course_files::{CanvasFile, download_file};
use super::pdf_analysis::load_pdf_pages;
use super::{MAX_OUTPUT_CHARS, clip};

const HOMEWORK_PREAMBLE_CHARS: usize = 800;
const HOMEWORK_OUTLINE_CHARS: usize = 150;

static HW_KEYWORD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?i)(?:problem|question|exercise|task|hw|q|习题|题目|练习)\s*[#.:：]?\s*(\d{1,3}(?:\.\d{1,3})?)(?:\D|$)").unwrap()
});
static HW_CHINESE_TI: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^第\s*[一二三四五六七八九十百零〇两\d]{1,4}\s*[题问]").unwrap());
static HW_SECTION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[一二三四五六七八九十]{1,3}\s*[、.．]").unwrap());
static HW_DOTTED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^((?:\d{1,2}\.){1,3})(\d{1,3})\.?(?:\s|$)").unwrap());
static HW_ARABIC: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\d{1,2})\s*[.．、)）]\s*(\S?)").unwrap());
static HW_PAREN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[(（]\s*(\d{1,2})\s*[)）]").unwrap());
static HW_SUBPART: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:[(（]\s*([a-hA-H]|i{1,3}|iv|vi{0,3})\s*[)）]|([a-h])[.)]\s)").unwrap()
});
static HW_POINTS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)[(（\[【]\s*(\d+(?:\.\d+)?)\s*(?:points?|pts?\.?|marks?|分)\s*[)）\]】]").unwrap()
});
static HW_FILE_LINK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"/(?:courses/\d+/)?files/(\d+)").unwrap());

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum QuestionStyle {
    Keyword,
    ChineseTi,
    Dotted,
    Arabic,
    Paren,
    Section,
    Whole,
}

impl QuestionStyle {
    fn describe(self) -> &'static str {
        match self {
            QuestionStyle::Keyword => "Problem/Question/习题 N",
            QuestionStyle::ChineseTi => "第N题",
            QuestionStyle::Dotted => "2.1.1 / 3.4",
            QuestionStyle::Arabic => "1. / 1) / 1、",
            QuestionStyle::Paren => "(1)",
            QuestionStyle::Section => "一、二、",
            QuestionStyle::Whole => "未识别到编号，整体视为一题",
        }
    }
}

struct QuestionHeader {
    line: usize,
    style: QuestionStyle,
    /// 多级编号的前缀，如 "2.1.1" 的 "2.1."；单级编号为空
    prefix: String,
    number: Option<u32>,
    label: String,
}

#[derive(Debug)]
struct Question {
    label: String,
    section: Option<String>,
    first_page: usize,
    last_page: usize,
    points: Option<String>,
    subparts: Vec<String>,
    text: String,
}

struct ParsedHomework {
    style: QuestionStyle,
    preamble: String,
    questions: Vec<Question>,
}

fn question_header(line: usize, text: &str) -> Option<QuestionHeader> {
    let header = |style, prefix: &str, number, label: &str| {
        Some(QuestionHeader {
            line,
            style,
            prefix: prefix.to_string(),
            number,
            label: label.trim().to_string(),
        })
    };
    if let Some(caps) = HW_KEYWORD.captures(text) {
        return header(QuestionStyle::Keyword, "", None, &text[..caps.get(1).unwrap().end()]);
    }
    if let Some(found) = HW_CHINESE_TI.find(text) {
        return header(QuestionStyle::ChineseTi, "", None, found.as_str());
    }
    if let Some(caps) = HW_PAREN.captures(text) {
        let number = caps[1].parse().ok();
        return header(QuestionStyle::Paren, "", number, caps.get(0).unwrap().as_str());
    }
    if let Some(caps) = HW_DOTTED.captures(text) {
        let label = format!("{}{}", &caps[1], &caps[2]);
        return header(QuestionStyle::Dotted, &caps[1], caps[2].parse().ok(), &label);
    }
    if let Some(caps) = HW_ARABIC.captures(text) {
        // "3.5 m/s" 这类小数不是题号
        if caps[2].chars().next().is_some_and(|c| c.is_ascii_digit()) {
            return None;
        }
        return header(QuestionStyle::Arabic, "", caps[1].parse().ok(), &caps[1]);
    }
    None
}

/// 取编号逐一递增的最长链；中间出现「一、二、」这类大题标题，或多级编号换了章节
/// （2.1.3 → 2.2.1）时允许从 1 重新编号。
/// 这样能过滤掉题干内部的列举、年份、正文里碰巧出现在行首的数字。
fn longest_chain<'a>(
    candidates: &[&'a QuestionHeader],
    section_lines: &[usize],
) -> Vec<&'a QuestionHeader> {
    let mut best: Vec<&QuestionHeader> = Vec::new();
    for start in 0..candidates.len() {
        let mut chain = vec![candidates[start]];
        for candidate in &candidates[start + 1..] {
            let last = chain.last().unwrap();
            let same_prefix = candidate.prefix == last.prefix;
            let restart = candidate.number == Some(1)
                && (!same_prefix
                    || section_lines.iter().any(|&line| line > last.line && line < candidate.line));
            if (same_prefix && candidate.number == last.number.map(|n| n + 1)) || restart {
                chain.push(candidate);
            }
        }
        if chain.len() > best.len() {
            best = chain;
        }
    }
    best
}

fn split_questions(pages: &[String]) -> ParsedHomework {
    let lines: Vec<(usize, &str)> = pages
        .iter()
        .enumerate()
        .flat_map(|(page, text)| text.lines().map(move |line| (page + 1, line.trim())))
        .filter(|(_, line)| !line.is_empty())
        .collect();

    let headers: Vec<QuestionHeader> = lines
        .iter()
        .enumerate()
        .filter_map(|(index, (_, text))| question_header(index, text))
        .collect();
    let sections: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, (_, text))| HW_SECTION.is_match(text))
        .map(|(index, _)| index)
        .collect();

    let of_style = |style| headers.iter().filter(|h| h.style == style).collect::<Vec<_>>();
    let keyword = of_style(QuestionStyle::Keyword);
    let chinese = of_style(QuestionStyle::ChineseTi);
    // 多级编号只看最深的一层，"2.1" 这类上层编号是小节标题
    let dotted_all = of_style(QuestionStyle::Dotted);
    let depth = |h: &QuestionHeader| h.prefix.matches('.').count();
    let deepest = dotted_all.iter().map(|h| depth(h)).max().unwrap_or(0);
    let dotted = longest_chain(
        &dotted_all.iter().copied().filter(|h| depth(h) == deepest).collect::<Vec<_>>(),
        &sections,
    );
    let arabic = longest_chain(&of_style(QuestionStyle::Arabic), &sections);
    let paren = longest_chain(&of_style(QuestionStyle::Paren), &sections);

    // (行号, 题号标签)
    let (style, mut starts): (QuestionStyle, Vec<(usize, String)>) = if !keyword.is_empty() {
        (QuestionStyle::Keyword, keyword.iter().map(|h| (h.line, h.label.clone())).collect())
    } else if !chinese.is_empty() {
        (QuestionStyle::ChineseTi, chinese.iter().map(|h| (h.line, h.label.clone())).collect())
    } else if dotted.len() >= 2 && dotted.len() >= arabic.len() {
        (QuestionStyle::Dotted, dotted.iter().map(|h| (h.line, h.label.clone())).collect())
    } else if arabic.len() >= 2 && arabic.len() >= paren.len() {
        (QuestionStyle::Arabic, arabic.iter().map(|h| (h.line, h.label.clone())).collect())
    } else if paren.len() >= 2 {
        (QuestionStyle::Paren, paren.iter().map(|h| (h.line, h.label.clone())).collect())
    } else if sections.len() >= 2 {
        let starts = sections
            .iter()
            .map(|&line| (line, HW_SECTION.find(lines[line].1).unwrap().as_str().to_string()))
            .collect();
        (QuestionStyle::Section, starts)
    } else {
        (QuestionStyle::Whole, vec![(0, "全文".to_string())])
    };

    // 同一题跨页时常重复题头（如 "Problem 2 (continued)"），合并到同一题
    starts.dedup_by(|next, previous| style == QuestionStyle::Keyword && next.1 == previous.1);

    let section_label = |line: usize| -> Option<String> {
        if style == QuestionStyle::Section {
            return None;
        }
        sections
            .iter()
            .rev()
            .find(|&&section| section < line)
            .map(|&section| lines[section].1.chars().take(40).collect())
    };

    let preamble_end = starts.first().map(|(line, _)| *line).unwrap_or(0);
    let preamble: String = lines[..preamble_end]
        .iter()
        .filter(|(_, text)| !HW_SECTION.is_match(text))
        .map(|(_, text)| *text)
        .collect::<Vec<_>>()
        .join("\n");

    let mut questions = Vec::new();
    for (position, (start, label)) in starts.iter().enumerate() {
        let next_start = starts.get(position + 1).map(|(line, _)| *line).unwrap_or(lines.len());
        // 下一道大题标题之前就结束，避免把「二、填空题」算进上一题
        let end = sections
            .iter()
            .copied()
            .find(|&line| line > *start && line < next_start && style != QuestionStyle::Section)
            .unwrap_or(next_start);
        let body = &lines[*start..end];
        if body.is_empty() {
            continue;
        }

        let text = body.iter().map(|(_, text)| *text).collect::<Vec<_>>().join("\n");
        let head: String = text.chars().take(300).collect();
        let points = HW_POINTS.captures(&head).map(|caps| caps[1].to_string());

        let mut subparts: Vec<String> = Vec::new();
        for (_, line) in &body[1..] {
            let found = HW_SUBPART
                .captures(line)
                .map(|caps| format!("({})", caps.get(1).or(caps.get(2)).unwrap().as_str()))
                .or_else(|| {
                    (style != QuestionStyle::Paren)
                        .then(|| HW_PAREN.captures(line).map(|caps| format!("({})", &caps[1])))
                        .flatten()
                });
            if let Some(found) = found {
                if !subparts.contains(&found) {
                    subparts.push(found);
                }
            }
        }

        questions.push(Question {
            label: label.clone(),
            section: section_label(*start),
            first_page: body.first().unwrap().0,
            last_page: body.last().unwrap().0,
            points,
            subparts,
            text,
        });
    }

    ParsedHomework { style, preamble, questions }
}

fn question_title(index: usize, question: &Question) -> String {
    let mut title = format!("第 {} 题 · {}", index + 1, question.label);
    if let Some(section) = &question.section {
        title.push_str(&format!(" · 所属：{section}"));
    }
    if question.first_page == question.last_page {
        title.push_str(&format!(" · 第 {} 页", question.first_page));
    } else {
        title.push_str(&format!(" · 第 {}–{} 页", question.first_page, question.last_page));
    }
    if let Some(points) = &question.points {
        title.push_str(&format!(" · {points} 分"));
    }
    if !question.subparts.is_empty() {
        title.push_str(&format!(" · 小问 {}", question.subparts.join("")));
    }
    title
}

fn render_questions(header: &str, parsed: &ParsedHomework, only: Option<&str>) -> Result<String, String> {
    if let Some(wanted) = only {
        let wanted = wanted.trim().to_lowercase();
        let found = parsed.questions.iter().enumerate().find(|(index, question)| {
            (index + 1).to_string() == wanted || question.label.to_lowercase() == wanted
        });
        return match found {
            Some((index, question)) => Ok(clip(&format!(
                "{header}\n=== {} ===\n{}\n",
                question_title(index, question),
                question.text
            ))),
            None => Err(format!(
                "没有编号为「{wanted}」的题目，共识别到 {} 题，可传 1–{} 的序号",
                parsed.questions.len(),
                parsed.questions.len()
            )),
        };
    }

    let mut text = format!(
        "{header}识别到 {} 道题（编号样式：{}）\n",
        parsed.questions.len(),
        parsed.style.describe()
    );
    let preamble = parsed.preamble.trim();
    if !preamble.is_empty() {
        let preamble: String = preamble.chars().take(HOMEWORK_PREAMBLE_CHARS).collect();
        text.push_str(&format!("\n[题前说明]\n{preamble}\n"));
    }

    let mut full = text.clone();
    for (index, question) in parsed.questions.iter().enumerate() {
        full.push_str(&format!("\n=== {} ===\n{}\n", question_title(index, question), question.text));
    }
    if full.len() < MAX_OUTPUT_CHARS {
        return Ok(full);
    }

    text.push_str("\n内容较长，以下为题目目录：\n");
    for (index, question) in parsed.questions.iter().enumerate() {
        let head: String = question
            .text
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(HOMEWORK_OUTLINE_CHARS)
            .collect();
        text.push_str(&format!("\n{}\n  {head}\n", question_title(index, question)));
    }
    text.push_str("\n请用 question=序号 逐题读取完整题干。");
    Ok(clip(&text))
}

fn decode_entity(entity: &str) -> Option<String> {
    let named = match entity {
        "nbsp" => " ",
        "amp" => "&",
        "lt" => "<",
        "gt" => ">",
        "quot" => "\"",
        "apos" => "'",
        _ => "",
    };
    if !named.is_empty() {
        return Some(named.to_string());
    }
    let code = entity.strip_prefix('#')?;
    let value = match code.strip_prefix(['x', 'X']) {
        Some(hex) => u32::from_str_radix(hex, 16).ok()?,
        None => code.parse().ok()?,
    };
    char::from_u32(value).map(String::from)
}

/// Canvas 作业说明是 HTML。转成纯文本时保留换行和 <ol> 的编号，题号识别依赖它们。
fn html_to_text(html: &str) -> String {
    let mut out = String::new();
    let mut lists: Vec<Option<u32>> = Vec::new();
    let mut rest = html;

    while let Some(position) = rest.find(['<', '&']) {
        out.push_str(&rest[..position]);
        rest = &rest[position..];

        if rest.starts_with('&') {
            match rest.find(';').filter(|&end| end <= 10) {
                Some(end) => match decode_entity(&rest[1..end]) {
                    Some(decoded) => {
                        out.push_str(&decoded);
                        rest = &rest[end + 1..];
                    }
                    None => {
                        out.push('&');
                        rest = &rest[1..];
                    }
                },
                None => {
                    out.push('&');
                    rest = &rest[1..];
                }
            }
            continue;
        }

        let Some(end) = rest.find('>') else {
            break;
        };
        let tag = rest[1..end].trim().to_lowercase();
        rest = &rest[end + 1..];
        let closing = tag.starts_with('/');
        let name: String = tag
            .trim_start_matches('/')
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect();

        match (name.as_str(), closing) {
            ("ol", false) => lists.push(Some(0)),
            ("ul", false) => lists.push(None),
            ("ol" | "ul", true) => {
                lists.pop();
                out.push('\n');
            }
            ("li", false) => {
                out.push('\n');
                match lists.last_mut() {
                    Some(Some(counter)) => {
                        *counter += 1;
                        out.push_str(&format!("{counter}. "));
                    }
                    _ => out.push_str("- "),
                }
            }
            ("br", _) => out.push('\n'),
            ("p" | "div" | "tr" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "pre" | "blockquote", _) => {
                out.push('\n')
            }
            ("td" | "th", true) => out.push('\t'),
            ("img", _) => out.push_str("[图片]"),
            _ => {}
        }
    }
    out.push_str(rest);

    let mut text = String::new();
    let mut blank = 0;
    for line in out.lines().map(str::trim_end) {
        if line.trim().is_empty() {
            blank += 1;
            if blank > 1 {
                continue;
            }
        } else {
            blank = 0;
        }
        text.push_str(line);
        text.push('\n');
    }
    text.trim().to_string()
}

fn text_to_html(text: &str) -> String {
    if text.trim_start().starts_with('<') {
        return text.to_string();
    }
    let escape = |line: &str| {
        let indent = line.len() - line.trim_start().len();
        let body = line
            .trim_start()
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;");
        format!("{}{body}", "&nbsp;".repeat(indent))
    };
    text.replace("\r\n", "\n")
        .split("\n\n")
        .filter(|paragraph| !paragraph.trim().is_empty())
        .map(|paragraph| {
            let lines: Vec<String> = paragraph.lines().map(escape).collect();
            format!("<p>{}</p>", lines.join("<br>\n"))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[derive(serde::Deserialize)]
struct HomeworkAssignment {
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    due_at: Option<String>,
    #[serde(default)]
    points_possible: Option<f64>,
    #[serde(default)]
    submission_types: Option<Vec<String>>,
    #[serde(default)]
    allowed_extensions: Option<Vec<String>>,
    #[serde(default)]
    locked_for_user: bool,
    #[serde(default)]
    lock_explanation: Option<String>,
    #[serde(default)]
    html_url: Option<String>,
}

impl HomeworkAssignment {
    fn summary(&self) -> String {
        let mut text = format!("作业：{}\n截止：{}\n", self.name, self.due_at.as_deref().unwrap_or("未设置"));
        if let Some(points) = self.points_possible {
            text.push_str(&format!("分值：{points}\n"));
        }
        if let Some(types) = self.submission_types.as_ref().filter(|types| !types.is_empty()) {
            text.push_str(&format!("提交方式：{}\n", types.join("、")));
        }
        if let Some(extensions) = self.allowed_extensions.as_ref().filter(|ext| !ext.is_empty()) {
            text.push_str(&format!("允许的文件类型：{}\n", extensions.join("、")));
        }
        if let Some(url) = &self.html_url {
            text.push_str(&format!("链接：{url}\n"));
        }
        text
    }
}

fn canvas_token() -> Result<String, String> {
    std::env::var("CANVAS_API_TOKEN")
        .map_err(|_| "canvas apitoken not found ,please check your dotenv config".to_string())
}

async fn canvas_failure(response: reqwest::Response, action: &str) -> String {
    let status = response.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        return "认证失败 (401)：令牌无效或已过期".to_string();
    }
    let body = response.text().await.unwrap_or_default();
    let body: String = body.chars().take(500).collect();
    format!("{action}失败，Canvas 返回 {status}：{body}")
}

async fn fetch_homework_assignment(
    client: &reqwest::Client,
    token: &str,
    course_id: &str,
    assignment_id: &str,
) -> Result<HomeworkAssignment, String> {
    let response = client
        .get(format!("https://oc.sjtu.edu.cn/api/v1/courses/{course_id}/assignments/{assignment_id}"))
        .bearer_auth(token)
        .send()
        .await
        .map_err(|err| format!("请求 Canvas 失败: {err}"))?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Err(format!("课程 {course_id} 中没有 id 为 {assignment_id} 的作业，可先用 get_exam 查看"));
    }
    if !response.status().is_success() {
        return Err(canvas_failure(response, "读取作业").await);
    }
    response
        .json()
        .await
        .map_err(|err| format!("解析作业详情失败: {err}"))
}

fn homework_arg<'a>(arguments: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    arguments[key]
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

/// 下载作业说明里链接的附件，返回 (本地路径, 是否 PDF) 和下载失败的说明。
async fn download_homework_attachments(
    client: &reqwest::Client,
    token: &str,
    assignment_id: &str,
    html: &str,
) -> (Vec<(PathBuf, bool)>, Vec<String>) {
    let mut ids: Vec<&str> = Vec::new();
    for caps in HW_FILE_LINK.captures_iter(html) {
        let id = caps.get(1).unwrap().as_str();
        if !ids.contains(&id) {
            ids.push(id);
        }
    }

    let dir = std::env::temp_dir().join("jisjtu-homework").join(assignment_id);
    let mut saved = Vec::new();
    let mut failures = Vec::new();
    for id in ids {
        let file = match client
            .get(format!("https://oc.sjtu.edu.cn/api/v1/files/{id}"))
            .bearer_auth(token)
            .send()
            .await
            .and_then(|response| response.error_for_status())
        {
            Ok(response) => match response.json::<CanvasFile>().await {
                Ok(file) => file,
                Err(err) => {
                    failures.push(format!("附件 {id} 信息解析失败：{err}"));
                    continue;
                }
            },
            Err(err) => {
                failures.push(format!("附件 {id} 获取失败：{err}"));
                continue;
            }
        };
        let target = dir.join(file.local_name());
        match download_file(client, token, &file, &target).await {
            Ok(_) => {
                let is_pdf = file.content_type.as_deref().is_some_and(|kind| kind.contains("pdf"))
                    || file.local_name().to_lowercase().ends_with(".pdf");
                saved.push((target, is_pdf));
            }
            Err(err) => failures.push(format!("附件 {} 下载失败：{err}", file.name())),
        }
    }
    (saved, failures)
}

async fn homework_questions(arguments: &serde_json::Value) -> Result<String, String> {
    let only = homework_arg(arguments, "question");

    if let Some(text) = homework_arg(arguments, "text") {
        let parsed = split_questions(&[text.to_string()]);
        return render_questions("来源：用户提供的作业文字\n", &parsed, only);
    }

    if let Some(path) = homework_arg(arguments, "path") {
        let is_pdf = path.to_lowercase().ends_with(".pdf")
            || std::fs::read(path).map(|bytes| bytes.starts_with(b"%PDF")).unwrap_or(false);
        let pages: Vec<String> = if is_pdf {
            load_pdf_pages(path).await?.as_ref().clone()
        } else {
            vec![tokio::fs::read_to_string(path)
                .await
                .map_err(|err| format!("读取 {path} 失败: {err}"))?]
        };
        let header = format!("来源：{path}（共 {} 页）\n", pages.len());
        return render_questions(&header, &split_questions(&pages), only);
    }

    let (Some(course_id), Some(assignment_id)) = (
        homework_arg(arguments, "course_id"),
        homework_arg(arguments, "assignment_id"),
    ) else {
        return Err("请提供题目来源：path（本地 PDF/文本）、text（作业文字）或 course_id + assignment_id（Canvas 作业）".to_string());
    };

    let token = canvas_token()?;
    let client = reqwest::Client::new();
    let assignment = fetch_homework_assignment(&client, &token, course_id, assignment_id).await?;
    let html = assignment.description.clone().unwrap_or_default();
    let description = html_to_text(&html);
    let (attachments, failures) =
        download_homework_attachments(&client, &token, assignment_id, &html).await;

    let mut header = assignment.summary();
    for (path, _) in &attachments {
        header.push_str(&format!("附件已下载：{}\n", path.display()));
    }
    for failure in &failures {
        header.push_str(&format!("{failure}\n"));
    }

    let mut pages: Vec<String> = Vec::new();
    for (path, _) in attachments.iter().filter(|(_, is_pdf)| *is_pdf) {
        let path = path.to_string_lossy();
        match load_pdf_pages(&path).await {
            Ok(loaded) => {
                header.push_str(&format!(
                    "{path} 对应第 {}–{} 页\n",
                    pages.len() + 1,
                    pages.len() + loaded.len()
                ));
                pages.extend(loaded.iter().cloned());
            }
            Err(err) => header.push_str(&format!("{path} 无法提取文字：{err}\n")),
        }
    }

    if pages.is_empty() {
        if description.is_empty() {
            return Ok(format!(
                "{header}\n作业说明为空，也没有可读取的 PDF 附件。其他附件请用对应工具读取。"
            ));
        }
        header.push_str("题目来源：作业说明文字\n");
        return render_questions(&header, &split_questions(&[description]), only);
    }

    if !description.is_empty() {
        let description: String = description.chars().take(HOMEWORK_PREAMBLE_CHARS).collect();
        header.push_str(&format!("\n[作业说明]\n{description}\n"));
    }
    header.push_str("题目来源：PDF 附件\n");
    render_questions(&header, &split_questions(&pages), only)
}

async fn upload_submission_file(
    client: &reqwest::Client,
    token: &str,
    course_id: &str,
    assignment_id: &str,
    path: &Path,
) -> Result<u64, String> {
    #[derive(serde::Deserialize)]
    struct UploadTicket {
        upload_url: String,
        #[serde(default)]
        upload_params: serde_json::Map<String, serde_json::Value>,
    }

    let bytes = tokio::fs::read(path)
        .await
        .map_err(|err| format!("读取 {} 失败: {err}", path.display()))?;
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "submission".to_string());
    let content_type = mime_guess::from_path(path).first_or_octet_stream().to_string();

    let response = client
        .post(format!(
            "https://oc.sjtu.edu.cn/api/v1/courses/{course_id}/assignments/{assignment_id}/submissions/self/files"
        ))
        .bearer_auth(token)
        .form(&[
            ("name", name.clone()),
            ("size", bytes.len().to_string()),
            ("content_type", content_type.clone()),
        ])
        .send()
        .await
        .map_err(|err| format!("请求上传地址失败: {err}"))?;
    if !response.status().is_success() {
        return Err(canvas_failure(response, &format!("申请上传 {name} ")).await);
    }
    let ticket: UploadTicket = response
        .json()
        .await
        .map_err(|err| format!("解析上传地址失败: {err}"))?;

    let mut form = reqwest::multipart::Form::new();
    for (key, value) in ticket.upload_params {
        let value = match value {
            serde_json::Value::String(text) => text,
            other => other.to_string(),
        };
        form = form.text(key, value);
    }
    let part = reqwest::multipart::Part::bytes(bytes)
        .file_name(name.clone())
        .mime_str(&content_type)
        .map_err(|err| err.to_string())?;
    form = form.part("file", part);

    // 上传完成后的 3XX 必须由我们带着令牌 GET 一次才算确认，不能交给自动重定向
    let uploader = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|err| err.to_string())?;
    let response = uploader
        .post(&ticket.upload_url)
        .multipart(form)
        .send()
        .await
        .map_err(|err| format!("上传 {name} 失败: {err}"))?;

    // Canvas 的第三步有两种形式：3XX 跳到确认地址；或 201 直接给出文件，
    // 有的存储后端（inst-fs）在 201 里只给 location，也要再 GET 一次才算确认。
    let status = response.status();
    if !status.is_success() && !status.is_redirection() {
        return Err(canvas_failure(response, &format!("上传 {name} ")).await);
    }
    let header_location = response
        .headers()
        .get("location")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let body: serde_json::Value = if status.is_redirection() {
        serde_json::Value::Null
    } else {
        response.json().await.unwrap_or(serde_json::Value::Null)
    };
    if let Some(id) = body["id"].as_u64() {
        return Ok(id);
    }
    let location = body["location"]
        .as_str()
        .map(str::to_string)
        .or(header_location)
        .ok_or_else(|| format!("上传 {name} 后既没有文件 id 也没有确认地址：{body}"))?;
    let location = reqwest::Url::parse(&ticket.upload_url)
        .and_then(|base| base.join(&location))
        .map_err(|err| format!("上传 {name} 的确认地址无效（{location}）: {err}"))?;

    let confirm = client.get(location.clone());
    let confirm = if location.host_str() == Some("oc.sjtu.edu.cn") {
        confirm.bearer_auth(token)
    } else {
        confirm
    };
    let response = confirm
        .send()
        .await
        .map_err(|err| format!("确认上传 {name} 失败: {err}"))?;
    if !response.status().is_success() {
        return Err(canvas_failure(response, &format!("确认上传 {name} ")).await);
    }
    let file: serde_json::Value = response
        .json()
        .await
        .map_err(|err| format!("解析上传结果失败: {err}"))?;
    file["id"]
        .as_u64()
        .ok_or_else(|| format!("上传 {name} 后没有拿到文件 id：{file}"))
}

/// 工具结果里这条分隔线之后的内容由聊天循环直接展示给用户，不进入模型上下文。
pub const USER_NOTICE_MARK: &str = "\n<<<仅展示给用户>>>\n";
const HOMEWORK_CONFIRM_MINUTES: u64 = 30;

/// 等待用户确认的提交。真正提交时只用这里保存的内容，模型无法在确认后替换答案。
struct PendingSubmission {
    course_id: String,
    assignment_id: String,
    text: Option<String>,
    files: Vec<PathBuf>,
    files_digest: u64,
    comment: Option<String>,
    code: String,
    approved: bool,
    created: std::time::Instant,
}

impl PendingSubmission {
    fn expired(&self) -> bool {
        self.created.elapsed().as_secs() > HOMEWORK_CONFIRM_MINUTES * 60
    }
}

fn pending_submissions() -> &'static Mutex<Vec<PendingSubmission>> {
    static PENDING: OnceLock<Mutex<Vec<PendingSubmission>>> = OnceLock::new();
    PENDING.get_or_init(|| Mutex::new(Vec::new()))
}

/// 由聊天循环在每轮开始时用用户本人输入的原文调用：只有用户亲手输入确认码，
/// 对应的提交才会放行。模型无法伪造用户消息，因此不能替用户确认。
pub fn approve_homework_submissions(user_prompt: &str) {
    let mut pending = pending_submissions().lock().unwrap();
    pending.retain(|submission| !submission.expired());
    for submission in pending.iter_mut() {
        if user_prompt.contains(&submission.code) {
            submission.approved = true;
        }
    }
}

fn confirmation_code() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or_default(),
    );
    format!("{:06}", hasher.finish() % 1_000_000)
}

fn files_digest(files: &[PathBuf]) -> Result<u64, String> {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for file in files {
        file.hash(&mut hasher);
        std::fs::read(file)
            .map_err(|err| format!("读取 {} 失败: {err}", file.display()))?
            .hash(&mut hasher);
    }
    Ok(hasher.finish())
}

fn check_submittable(assignment: &HomeworkAssignment, kind: &str, files: &[PathBuf]) -> Result<(), String> {
    if assignment.locked_for_user {
        return Err(format!(
            "作业「{}」当前不可提交：{}",
            assignment.name,
            assignment.lock_explanation.as_deref().unwrap_or("已锁定")
        ));
    }
    let types = assignment.submission_types.clone().unwrap_or_default();
    if !types.iter().any(|allowed| allowed == kind) {
        return Err(format!(
            "作业「{}」不接受 {kind}，允许的提交方式：{}",
            assignment.name,
            if types.is_empty() { "无".to_string() } else { types.join("、") }
        ));
    }
    if let Some(extensions) = assignment.allowed_extensions.as_ref().filter(|ext| !ext.is_empty()) {
        for file in files {
            let extension = file
                .extension()
                .map(|ext| ext.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            if !extensions.iter().any(|allowed| allowed.to_lowercase() == extension) {
                return Err(format!(
                    "{} 的类型不被允许，该作业只接受：{}",
                    file.display(),
                    extensions.join("、")
                ));
            }
        }
    }
    Ok(())
}

async fn preview_homework(arguments: &serde_json::Value, course_id: &str, assignment_id: &str) -> Result<String, String> {
    let text = homework_arg(arguments, "text");
    let files: Vec<PathBuf> = arguments["files"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::trim).filter(|path| !path.is_empty()))
                .map(PathBuf::from)
                .collect()
        })
        .unwrap_or_default();
    let comment = homework_arg(arguments, "comment");

    let kind = match (text.is_some(), files.is_empty()) {
        (true, false) => return Err("text 和 files 只能二选一：Canvas 一次提交只能是在线文本或文件上传".to_string()),
        (false, true) => return Err("请提供要提交的内容：text（在线文本）或 files（本地文件路径）".to_string()),
        (true, true) => "online_text_entry",
        (false, false) => "online_upload",
    };
    for file in &files {
        if !file.is_file() {
            return Err(format!("文件 {} 不存在", file.display()));
        }
    }

    let token = canvas_token()?;
    let client = reqwest::Client::new();
    let assignment = fetch_homework_assignment(&client, &token, course_id, assignment_id).await?;
    check_submittable(&assignment, kind, &files)?;

    let code = confirmation_code();
    let mut notice = format!("【作业提交预览，尚未提交】\n{}本次提交方式：{kind}\n", assignment.summary());
    if let Some(text) = text {
        notice.push_str(&format!("\n提交内容（共 {} 字）：\n{text}\n", text.chars().count()));
    }
    for file in &files {
        let size = std::fs::metadata(file).map(|meta| meta.len()).unwrap_or(0);
        notice.push_str(&format!("\n提交文件：{}（{size} 字节）", file.display()));
    }
    if let Some(comment) = comment {
        notice.push_str(&format!("\n附言：{comment}\n"));
    }
    notice.push_str(&format!(
        "\n请核对以上内容。确认无误后回复：确认提交 {code}\n确认码 {HOMEWORK_CONFIRM_MINUTES} 分钟内有效，不回复则不会提交。"
    ));

    let submission = PendingSubmission {
        course_id: course_id.to_string(),
        assignment_id: assignment_id.to_string(),
        text: text.map(str::to_string),
        files_digest: files_digest(&files)?,
        files,
        comment: comment.map(str::to_string),
        code,
        approved: false,
        created: std::time::Instant::now(),
    };
    {
        let mut pending = pending_submissions().lock().unwrap();
        // 同一作业只保留最新一次预览，旧确认码随之作废
        pending.retain(|old| {
            !old.expired() && !(old.course_id == course_id && old.assignment_id == assignment_id)
        });
        pending.push(submission);
    }

    Ok(format!(
        "提交预览和确认码已由系统直接展示给用户，作业尚未提交。\
         请简短提醒用户核对预览并回复确认码，然后结束本轮回答，等待用户回复。\
         用户回复确认码后，用 action=submit、confirm=true 和相同的 course_id、assignment_id 提交，\
         届时会提交用户确认过的那份内容。不要替用户确认，也不要编造确认码。\
         如需修改内容，请重新预览。{USER_NOTICE_MARK}{notice}"
    ))
}

async fn submit_homework(arguments: &serde_json::Value) -> Result<String, String> {
    let course_id = homework_arg(arguments, "course_id")
        .ok_or_else(|| "提交作业需要 course_id".to_string())?;
    let assignment_id = homework_arg(arguments, "assignment_id")
        .ok_or_else(|| "提交作业需要 assignment_id，可由 get_exam 获取".to_string())?;
    if arguments["confirm"].as_bool() != Some(true) {
        return preview_homework(arguments, course_id, assignment_id).await;
    }

    let submission = {
        let mut pending = pending_submissions().lock().unwrap();
        pending.retain(|submission| !submission.expired());
        let position = pending
            .iter()
            .position(|submission| submission.course_id == course_id && submission.assignment_id == assignment_id)
            .ok_or_else(|| "这份作业没有待确认的提交（或确认码已过期），请先用 confirm=false 生成预览".to_string())?;
        if !pending[position].approved {
            return Err("用户还没有回复确认码，作业未提交。请等待用户亲自回复预览中的确认码，不要替用户确认".to_string());
        }
        pending.remove(position)
    };
    if files_digest(&submission.files)? != submission.files_digest {
        return Err("提交文件在预览后被修改，作业未提交，请重新预览并让用户确认".to_string());
    }

    let kind = if submission.text.is_some() { "online_text_entry" } else { "online_upload" };
    let token = canvas_token()?;
    let client = reqwest::Client::new();
    let assignment = fetch_homework_assignment(&client, &token, course_id, assignment_id).await?;
    check_submittable(&assignment, kind, &submission.files)?;

    let mut form: Vec<(&str, String)> = vec![("submission[submission_type]", kind.to_string())];
    if let Some(text) = &submission.text {
        form.push(("submission[body]", text_to_html(text)));
    }
    for file in &submission.files {
        let id = upload_submission_file(&client, &token, course_id, assignment_id, file).await?;
        form.push(("submission[file_ids][]", id.to_string()));
    }
    if let Some(comment) = &submission.comment {
        form.push(("comment[text_comment]", comment.clone()));
    }

    let response = client
        .post(format!(
            "https://oc.sjtu.edu.cn/api/v1/courses/{course_id}/assignments/{assignment_id}/submissions"
        ))
        .bearer_auth(&token)
        .form(&form)
        .send()
        .await
        .map_err(|err| format!("提交作业失败: {err}"))?;
    if !response.status().is_success() {
        return Err(canvas_failure(response, "提交作业").await);
    }
    let result: serde_json::Value = response
        .json()
        .await
        .map_err(|err| format!("解析提交结果失败: {err}"))?;

    let mut report = format!("已提交作业「{}」（内容为用户确认过的预览）\n", assignment.name);
    if let Some(time) = result["submitted_at"].as_str() {
        report.push_str(&format!("提交时间：{time}\n"));
    }
    if let Some(attempt) = result["attempt"].as_u64() {
        report.push_str(&format!("第 {attempt} 次提交\n"));
    }
    if result["late"].as_bool() == Some(true) {
        report.push_str("注意：本次提交已被标记为迟交\n");
    }
    if let Some(url) = result["preview_url"].as_str() {
        report.push_str(&format!("查看：{url}\n"));
    }
    Ok(report)
}

pub(super) async fn homework(arguments: serde_json::Value) -> Result<String, String> {
    match arguments["action"].as_str().unwrap_or("questions").trim() {
        "questions" => homework_questions(&arguments).await,
        "submit" => submit_homework(&arguments).await,
        other => Err(format!("未知的 action「{other}」，只支持 questions / submit")),
    }
}

#[cfg(test)]
mod tests;
