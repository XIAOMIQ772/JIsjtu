use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::collections::HashMap;

use super::{campus_time, clip, shuiyuan};

const BASE: &str = "https://shuiyuan.sjtu.edu.cn";
const DEFAULT_COUNT: usize = 8;
const MAX_COUNT: usize = 20;
const DETAIL_COUNT: usize = 3;

#[derive(Deserialize)]
struct TopicList {
    topic_list: Topics,
    #[serde(default)]
    users: Vec<User>,
}

#[derive(Deserialize)]
struct Topics {
    #[serde(default)]
    topics: Vec<Topic>,
}

#[derive(Deserialize)]
struct User {
    id: u64,
    username: String,
}

#[derive(Deserialize, Clone)]
struct Topic {
    id: u64,
    #[serde(default)]
    title: String,
    #[serde(default)]
    slug: String,
    #[serde(default)]
    posts_count: u64,
    #[serde(default)]
    reply_count: u64,
    #[serde(default)]
    like_count: u64,
    #[serde(default)]
    views: u64,
    #[serde(default)]
    created_at: Option<String>,
    #[serde(default)]
    last_posted_at: Option<String>,
    #[serde(default)]
    posters: Vec<Poster>,
}

#[derive(Deserialize, Clone)]
struct Poster {
    user_id: u64,
}

#[derive(Deserialize)]
struct TopicDetail {
    post_stream: PostStream,
}

#[derive(Deserialize)]
struct PostStream {
    #[serde(default)]
    posts: Vec<Post>,
}

#[derive(Deserialize)]
struct Post {
    #[serde(default)]
    username: String,
    #[serde(default)]
    cooked: String,
    #[serde(default)]
    like_count: u64,
    #[serde(default)]
    post_number: u64,
}

fn parse_time(value: &Option<String>) -> Option<DateTime<Utc>> {
    value
        .as_deref()
        .and_then(|text| DateTime::parse_from_rfc3339(text).ok())
        .map(|time| time.with_timezone(&Utc))
}

/// 热度：点赞、回复、浏览加权，再按最近活跃时间衰减，避免老帖长期霸榜。
fn hot_score(topic: &Topic, now: DateTime<Utc>) -> f64 {
    let base = topic.like_count as f64 * 3.0
        + topic.reply_count.max(topic.posts_count.saturating_sub(1)) as f64 * 2.0
        + topic.views as f64 / 50.0;
    let active = parse_time(&topic.last_posted_at).or_else(|| parse_time(&topic.created_at));
    let days = active.map_or(7.0, |time| (now - time).num_minutes().max(0) as f64 / 1440.0);
    base / (1.0 + days / 2.0)
}

fn strip_html(html: &str) -> String {
    let mut text = String::new();
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' if in_tag => {
                in_tag = false;
                text.push(' ');
            }
            _ if !in_tag => text.push(ch),
            _ => {}
        }
    }
    let text = text
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ");
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn shorten(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        text.to_string()
    } else {
        format!("{}…", text.chars().take(max_chars).collect::<String>())
    }
}

fn parse<T: serde::de::DeserializeOwned>(value: Result<serde_json::Value, String>) -> Result<T, String> {
    serde_json::from_value(value?).map_err(|err| format!("水源数据格式变化: {err}"))
}

fn requested_count(arguments: &serde_json::Value) -> Result<usize, String> {
    if !arguments.is_object() {
        return Err("get_hotnews 参数必须是对象".into());
    }
    match arguments.get("limit") {
        None | Some(serde_json::Value::Null) => Ok(DEFAULT_COUNT),
        Some(value) => match value.as_u64() {
            Some(n) if (1..=MAX_COUNT as u64).contains(&n) => Ok(n as usize),
            _ => Err(format!("limit 必须是 1 到 {MAX_COUNT} 之间的整数")),
        },
    }
}

pub(super) async fn get_hotnews(arguments: serde_json::Value) -> Result<String, String> {
    let count = requested_count(&arguments)?;

    // 最新帖与周榜在同一个后台页面里请求；周榜失败不影响主流程，只用最新帖子计算热度。
    let mut lists = shuiyuan::fetch_json_many(&["/latest.json".into(), "/top.json?period=weekly".into()]).await?;
    let top: Option<TopicList> = parse(lists.pop().expect("两个请求")).ok();
    let latest: TopicList = parse(lists.pop().expect("两个请求"))?;

    let mut names: HashMap<u64, String> = HashMap::new();
    let mut topics: HashMap<u64, Topic> = HashMap::new();
    for list in std::iter::once(&latest).chain(top.iter()) {
        for user in &list.users {
            names.insert(user.id, user.username.clone());
        }
        for topic in &list.topic_list.topics {
            topics.entry(topic.id).or_insert_with(|| topic.clone());
        }
    }
    if topics.is_empty() {
        return Err("没有读取到任何帖子".into());
    }

    let now = Utc::now();
    let mut ranked: Vec<(f64, Topic)> = topics
        .into_values()
        .map(|topic| (hot_score(&topic, now), topic))
        .collect();
    ranked.sort_by(|a, b| b.0.total_cmp(&a.0).then(b.1.id.cmp(&a.1.id)));
    let analyzed = ranked.len();
    ranked.truncate(count);

    // 前几名的楼主内容与热评也一次请求完
    let detail_paths: Vec<String> = ranked
        .iter()
        .take(DETAIL_COUNT)
        .map(|(_, topic)| format!("/t/{}.json", topic.id))
        .collect();
    let mut details: Vec<Result<TopicDetail, String>> = match shuiyuan::fetch_json_many(&detail_paths).await {
        Ok(values) => values.into_iter().map(parse).collect(),
        Err(error) => detail_paths.iter().map(|_| Err(error.clone())).collect(),
    };
    details.reverse();

    let beijing_now = campus_time::now();
    let mut report = format!(
        "水源热点简报（共分析 {analyzed} 个帖子，按热度取前 {} 个）\n",
        ranked.len()
    );
    for (rank, (score, topic)) in ranked.iter().enumerate() {
        let author = topic
            .posters
            .first()
            .and_then(|poster| names.get(&poster.user_id))
            .map_or("未知", String::as_str);
        report.push_str(&format!(
            "\n{}. {}\n   链接：{BASE}/t/{}/{}\n   发帖人：{}　回复：{}　点赞：{}　浏览：{}　热度：{:.0}\n   最近活跃：{}\n",
            rank + 1,
            topic.title,
            if topic.slug.is_empty() { "topic" } else { &topic.slug },
            topic.id,
            author,
            topic.posts_count.saturating_sub(1),
            topic.like_count,
            topic.views,
            score,
            topic
                .last_posted_at
                .as_deref()
                .and_then(campus_time::parse_time)
                .map_or("未知".to_string(), |time| campus_time::fmt_day_time(time, beijing_now)),
        ));
        if rank < DETAIL_COUNT {
            match details.pop() {
                Some(Ok(detail)) => append_detail(&mut report, &detail),
                Some(Err(error)) => report.push_str(&format!("   （详情读取失败：{error}）\n")),
                None => {}
            }
        }
    }
    Ok(clip(&report))
}

fn append_detail(report: &mut String, detail: &TopicDetail) {
    let posts = &detail.post_stream.posts;
    if let Some(first) = posts.first() {
        report.push_str(&format!(
            "   楼主内容：{}\n",
            shorten(&strip_html(&first.cooked), 300)
        ));
    }
    let mut replies: Vec<&Post> = posts.iter().filter(|post| post.post_number > 1).collect();
    replies.sort_by(|a, b| b.like_count.cmp(&a.like_count));
    for post in replies.into_iter().take(2) {
        report.push_str(&format!(
            "   热评（{}，{} 赞）：{}\n",
            post.username,
            post.like_count,
            shorten(&strip_html(&post.cooked), 150)
        ));
    }
}
