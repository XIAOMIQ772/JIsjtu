//! degree_progress：学业完成情况（教务系统“学生修业情况查询”）。按培养方案的各类要求列出
//! 要求学分、已获得学分和还差多少，回答“离毕业还差什么”。
//!
//! 页面是服务端渲染的 HTML：每个大类是一个 <h4>（可能紧跟一条备注），每项要求是表头里的
//! 几个 <span>：名称、“要求 N 学分”“获得 M 学分”、可选的“备注：…”。

use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

use super::canvas::trim_number;
use super::clip;
use super::office::{Token, attr, tokens};
use crate::sso::{FetchRequest, campus_fetch};

const ENTRY: &str = "https://i.sjtu.edu.cn/jaccountlogin";
const AUDIT_PATH: &str = "/xjyj/xsxyqk_ckXsXyxxHtmlView.html?gnmkdm=N551225";

static REQUIRED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"要求\s*([\d.]+)\s*学分").unwrap());
static EARNED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"获得\s*([\d.]+)\s*学分").unwrap());
static BLOCKS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?is)<script\b.*?</script>|<style\b.*?</style>").unwrap());

#[derive(Debug, PartialEq)]
pub(super) struct Requirement {
    pub category: String,
    pub category_note: Option<String>,
    pub name: String,
    pub required: f64,
    pub earned: f64,
    pub note: Option<String>,
}

fn squash(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub(super) fn parse_audit(html: &str) -> Vec<Requirement> {
    let html = BLOCKS.replace_all(html, "");
    let mut requirements = Vec::new();
    let mut category = String::new();
    let mut category_note: Option<String> = None;
    let mut heading: Option<String> = None;
    let mut header: Option<Vec<String>> = None;
    let mut span: Option<String> = None;
    for token in tokens(&html) {
        match token {
            Token::Open { name, .. } if name.eq_ignore_ascii_case("h4") => heading = Some(String::new()),
            Token::Close(name) if name.eq_ignore_ascii_case("h4") => {
                let text = squash(&heading.take().unwrap_or_default());
                if let Some(note) = text.strip_prefix("备注：").or_else(|| text.strip_prefix("备注:")) {
                    category_note = Some(note.trim().to_string());
                } else if !text.is_empty() {
                    category = text;
                    category_note = None;
                }
            }
            Token::Open { name, attrs, .. } if name.eq_ignore_ascii_case("th") && attr(attrs, "colspan").is_some() => {
                header = Some(Vec::new());
            }
            Token::Close(name) if name.eq_ignore_ascii_case("th") => {
                let Some(spans) = header.take() else { continue };
                let joined = spans.join(" ");
                let (Some(required), Some(earned)) = (
                    REQUIRED.captures(&joined).and_then(|caps| caps[1].parse().ok()),
                    EARNED.captures(&joined).and_then(|caps| caps[1].parse().ok()),
                ) else {
                    continue;
                };
                let name = spans
                    .iter()
                    .find(|text| !text.contains("要求") && !text.contains("获得") && !text.starts_with("备注"))
                    .cloned()
                    .unwrap_or_default();
                let note = spans
                    .iter()
                    .find_map(|text| text.strip_prefix("备注：").or_else(|| text.strip_prefix("备注:")))
                    .map(|note| note.trim().to_string());
                requirements.push(Requirement {
                    category: category.clone(),
                    category_note: category_note.clone(),
                    name,
                    required,
                    earned,
                    note,
                });
            }
            Token::Open { name, .. } if name.eq_ignore_ascii_case("span") && header.is_some() => span = Some(String::new()),
            Token::Close(name) if name.eq_ignore_ascii_case("span") => {
                if let (Some(text), Some(spans)) = (span.take(), header.as_mut()) {
                    let text = squash(&text);
                    if !text.is_empty() {
                        spans.push(text);
                    }
                }
            }
            Token::Text(text) => {
                if let Some(span) = span.as_mut() {
                    span.push_str(&text);
                } else if let Some(heading) = heading.as_mut() {
                    heading.push_str(&text);
                }
            }
            _ => {}
        }
    }
    requirements
}

pub(super) fn render(requirements: &[Requirement]) -> String {
    if requirements.is_empty() {
        return "没有从教务系统读到培养方案要求（页面结构可能已变化），可以让用户在教学信息服务网“学生修业情况查询”中查看。".into();
    }
    let missing: Vec<&Requirement> = requirements
        .iter()
        .filter(|item| item.earned + 1e-9 < item.required)
        .collect();
    let gap: f64 = missing.iter().map(|item| item.required - item.earned).sum();
    let mut text = format!(
        "学业完成情况（教务系统·修业情况）：共 {} 项要求，已满足 {} 项，未满足 {} 项",
        requirements.len(),
        requirements.len() - missing.len(),
        missing.len()
    );
    if !missing.is_empty() {
        text.push_str(&format!("，合计还差 {} 学分", trim_number((gap * 10.0).round() / 10.0)));
    }
    text.push_str("\n“获得”只计已出成绩并通过的课程，本学期在修课程出分后才会计入。\n");

    let mut last_category: Option<&str> = None;
    for item in requirements {
        if last_category != Some(item.category.as_str()) {
            last_category = Some(item.category.as_str());
            if !item.category.is_empty() {
                text.push_str(&format!("\n【{}】", item.category));
                if let Some(note) = &item.category_note {
                    text.push_str(note);
                }
                text.push('\n');
            }
        }
        let status = if item.earned + 1e-9 >= item.required {
            "已满足".to_string()
        } else {
            format!("还差 {}", trim_number(((item.required - item.earned) * 10.0).round() / 10.0))
        };
        text.push_str(&format!(
            "- {}：要求 {} 学分，已获得 {}，{status}",
            if item.name.is_empty() { "（未命名）" } else { &item.name },
            trim_number(item.required),
            trim_number(item.earned)
        ));
        if let Some(note) = &item.note {
            text.push_str(&format!("（{note}）"));
        }
        text.push('\n');
    }
    if !missing.is_empty() {
        text.push_str("\n未满足的要求：\n");
        for item in &missing {
            let label = match (item.category.is_empty(), item.name.is_empty()) {
                (false, false) => format!("{} · {}", item.category, item.name),
                (false, true) => item.category.clone(),
                _ => item.name.clone(),
            };
            text.push_str(&format!(
                "- {label}：还差 {} 学分\n",
                trim_number(((item.required - item.earned) * 10.0).round() / 10.0)
            ));
        }
    }
    text.push_str("\n毕业资格以教务处审核为准；选课时可对照未满足的类别安排。");
    text
}

pub(super) async fn degree_progress(_: Value) -> Result<String, String> {
    // 这个页面是 HTML，不能走 eduinfo::query 的 JSON 解码
    let responses = campus_fetch(ENTRY, &[FetchRequest::get(AUDIT_PATH)], 2_000_000).await?;
    let response = responses.first().ok_or("浏览器没有返回页面")?;
    if let Some(error) = &response.error {
        return Err(format!("读取学业完成情况失败：{error}"));
    }
    if response.status != 200 {
        return Err(format!("教务系统返回 {}", response.status));
    }
    Ok(clip(&render(&parse_audit(&response.text))))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str = r#"
        <script>var x = "<h4>不是标题</h4>";</script>
        <th><span>毕业资格审核</span></th>
        <div class="ui-grid-handle">□</div> <h4>公共课程类</h4>
        <table class=" formlist"><thead><tr><th colspan="6">
            <span>必修</span>
            <span class='cleanInfoA'>要求45.5 学分</span>
            <span class='cleanInfoA'> 获得40.5 学分</span>
            <span style="width:100%">备注：须修满全部。</span>
        </th></tr></thead></table>
        <div class="ui-grid-handle">□</div> <h4>英语类</h4>
        <h4><span style="width:100%">备注：需选修 4 门，累计 8 学分</span></h4>
        <table class=" formlist"><thead><tr><th colspan="6">
            <span>英语一</span><span>要求2.0 学分</span><span> 获得2.0 学分</span>
        </th></tr></thead></table>
        <table class=" formlist"><thead><tr><th colspan="6">
            <span>英语二</span><span>要求2.0 学分</span><span> 获得0 学分</span>
        </th></tr></thead></table>
    "#;

    #[test]
    fn audit_page_becomes_requirements_by_category() {
        let items = parse_audit(PAGE);
        assert_eq!(items.len(), 3, "{items:?}");
        assert_eq!(items[0].category, "公共课程类");
        assert_eq!(items[0].name, "必修");
        assert_eq!((items[0].required, items[0].earned), (45.5, 40.5));
        assert_eq!(items[0].note.as_deref(), Some("须修满全部。"));
        assert_eq!(items[1].category, "英语类");
        assert_eq!(items[1].category_note.as_deref(), Some("需选修 4 门，累计 8 学分"));
        assert_eq!(items[2].name, "英语二");
        assert_eq!(items[2].earned, 0.0);
    }

    #[test]
    fn report_lists_gaps_and_totals() {
        let text = render(&parse_audit(PAGE));
        assert!(text.contains("共 3 项要求，已满足 1 项，未满足 2 项，合计还差 7 学分"), "{text}");
        assert!(text.contains("【公共课程类】\n- 必修：要求 45.5 学分，已获得 40.5，还差 5（须修满全部。）"), "{text}");
        assert!(text.contains("【英语类】需选修 4 门，累计 8 学分\n- 英语一：要求 2 学分，已获得 2，已满足"), "{text}");
        assert!(text.contains("未满足的要求：\n- 公共课程类 · 必修：还差 5 学分\n- 英语类 · 英语二：还差 2 学分"), "{text}");
        assert!(render(&[]).contains("没有从教务系统读到"));
    }
}
