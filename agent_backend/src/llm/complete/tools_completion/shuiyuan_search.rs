//! shuiyuan_search / shuiyuan_topic：搜索水源帖子（选课评价、老师口碑、校园经验），以及阅读某个帖子的楼层。

use chrono::{DateTime, FixedOffset};
use serde_json::Value;

use super::campus_time::{self, fmt_day_time, parse_time};
use super::canvas::null_default;
use super::clip;
use super::shuiyuan::{self, encode_query, topic_url};
use super::text::{html_to_text, one_line, shorten};

const DEFAULT_TOPICS: usize = 10;
const MAX_TOPICS: usize = 20;

#[derive(serde::Deserialize, Default)]
pub(super) struct SearchResult {
    #[serde(default, deserialize_with = "null_default")]
    posts: Vec<SearchPost>,
    #[serde(default, deserialize_with = "null_default")]
    topics: Vec<SearchTopic>,
}

#[derive(serde::Deserialize)]
struct SearchPost {
    #[serde(default)]
    username: String,
    #[serde(default)]
    created_at: Option<String>,
    #[serde(default)]
    like_count: u64,
    #[serde(default)]
    blurb: String,
    #[serde(default)]
    post_number: u64,
    #[serde(default)]
    topic_id: u64,
}

#[derive(serde::Deserialize)]
struct SearchTopic {
    id: u64,
    #[serde(default)]
    title: String,
    #[serde(default)]
    slug: String,
    #[serde(default)]
    posts_count: u64,
    #[serde(default)]
    like_count: u64,
    #[serde(default)]
    views: u64,
    #[serde(default)]
    created_at: Option<String>,
    #[serde(default)]
    last_posted_at: Option<String>,
    #[serde(default, deserialize_with = "null_default")]
    tags: Vec<Value>,
}

fn when(text: &Option<String>, now: DateTime<FixedOffset>) -> String {
    text.as_deref()
        .and_then(parse_time)
        .map_or("时间未知".into(), |time| fmt_day_time(time, now))
}

/// 标签可能是字符串，也可能是 {name: ...} 对象（新版 Discourse）。
fn tag_names(tags: &[Value]) -> Vec<String> {
    tags.iter()
        .filter_map(|tag| tag.as_str().or_else(|| tag["name"].as_str()).map(str::to_string))
        .collect()
}

pub(super) fn render_search(query: &str, result: &SearchResult, limit: usize, now: DateTime<FixedOffset>) -> String {
    if result.topics.is_empty() {
        return format!(
            "水源上没有搜到「{query}」相关的帖子。可以换个说法（课程简称、老师姓名），或去掉 order:latest 等过滤条件。"
        );
    }
    let mut text = format!(
        "水源搜索「{query}」：找到 {} 个相关帖子（显示前 {}），每个帖子下列出命中的楼层摘要\n",
        result.topics.len(),
        result.topics.len().min(limit)
    );
    for (index, topic) in result.topics.iter().take(limit).enumerate() {
        // 搜索结果里的帖子对象常常不带点赞和浏览数（为 0），只在有值时显示
        let mut facts = vec![format!("{} 楼", topic.posts_count)];
        if topic.like_count > 0 {
            facts.push(format!("{} 赞", topic.like_count));
        }
        if topic.views > 0 {
            facts.push(format!("{} 浏览", topic.views));
        }
        facts.push(format!("发布于 {}", when(&topic.created_at, now)));
        facts.push(format!("最近回复 {}", when(&topic.last_posted_at, now)));
        text.push_str(&format!(
            "\n{}. {}（帖子 id {}）\n   {}\n   {}",
            index + 1,
            topic.title.trim(),
            topic.id,
            topic_url(topic.id, &topic.slug),
            facts.join(" · ")
        ));
        let tags = tag_names(&topic.tags);
        if !tags.is_empty() {
            text.push_str(&format!(" · 标签 {}", tags.join("、")));
        }
        text.push('\n');
        for post in result.posts.iter().filter(|post| post.topic_id == topic.id).take(3) {
            text.push_str(&format!(
                "   - {} 楼 {}（{}，{} 赞）：{}\n",
                post.post_number,
                post.username,
                when(&post.created_at, now),
                post.like_count,
                shorten(&one_line(&html_to_text(&post.blurb)), 160)
            ));
        }
    }
    text.push_str("\n要看某个帖子的完整讨论，用 shuiyuan_topic 传帖子 id。注意帖子观点来自个人，可能过时，引用时请注明时间。");
    text
}

#[derive(serde::Deserialize)]
pub(super) struct TopicView {
    id: u64,
    #[serde(default)]
    title: String,
    #[serde(default)]
    slug: String,
    #[serde(default)]
    posts_count: u64,
    #[serde(default)]
    views: u64,
    #[serde(default)]
    like_count: u64,
    #[serde(default)]
    created_at: Option<String>,
    #[serde(default, deserialize_with = "null_default")]
    tags: Vec<Value>,
    post_stream: PostStream,
}

#[derive(serde::Deserialize)]
struct PostStream {
    #[serde(default, deserialize_with = "null_default")]
    posts: Vec<TopicPost>,
}

#[derive(serde::Deserialize)]
struct TopicPost {
    #[serde(default)]
    post_number: u64,
    #[serde(default)]
    username: String,
    #[serde(default)]
    created_at: Option<String>,
    #[serde(default)]
    cooked: String,
    #[serde(default)]
    like_count: u64,
    #[serde(default)]
    reply_to_post_number: Option<u64>,
}

pub(super) fn render_topic(topic: &TopicView, start: u64, now: DateTime<FixedOffset>) -> String {
    let mut posts: Vec<&TopicPost> = topic
        .post_stream
        .posts
        .iter()
        .filter(|post| post.post_number >= start)
        .collect();
    posts.sort_by_key(|post| post.post_number);
    let mut text = format!(
        "《{}》\n{}\n共 {} 楼 · {} 浏览 · {} 赞 · 发布于 {}",
        topic.title.trim(),
        topic_url(topic.id, &topic.slug),
        topic.posts_count,
        topic.views,
        topic.like_count,
        when(&topic.created_at, now)
    );
    let tags = tag_names(&topic.tags);
    if !tags.is_empty() {
        text.push_str(&format!(" · 标签 {}", tags.join("、")));
    }
    text.push('\n');
    if posts.is_empty() {
        text.push_str(&format!("第 {start} 楼之后没有更多回复。"));
        return text;
    }
    for post in &posts {
        let body = html_to_text(&post.cooked);
        let body: Vec<&str> = body.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
        let limit = if post.post_number == 1 { 1500 } else { 400 };
        let mut header = format!("\n—— {} 楼 {}（{}", post.post_number, post.username, when(&post.created_at, now));
        if post.like_count > 0 {
            header.push_str(&format!("，{} 赞", post.like_count));
        }
        if let Some(reply) = post.reply_to_post_number {
            header.push_str(&format!("，回复 {reply} 楼"));
        }
        text.push_str(&header);
        text.push_str("）——\n");
        text.push_str(&shorten(&body.join("\n"), limit));
        text.push('\n');
    }
    let last = posts.last().map_or(start, |post| post.post_number);
    if last < topic.posts_count {
        text.push_str(&format!(
            "\n已显示第 {}–{last} 楼，共 {} 楼；继续阅读请传 start={}。",
            posts.first().map_or(start, |post| post.post_number),
            topic.posts_count,
            last + 1
        ));
    }
    text
}

fn topic_id(value: &Value) -> Option<u64> {
    match value {
        Value::Number(number) => number.as_u64(),
        // 也接受帖子链接：https://shuiyuan.sjtu.edu.cn/t/topic/351551/20
        Value::String(text) => {
            let text = text.trim();
            text.parse().ok().or_else(|| {
                let after = text.split("/t/").nth(1)?;
                after.split('/').find_map(|part| part.parse().ok())
            })
        }
        _ => None,
    }
}

pub(super) async fn shuiyuan_search(arguments: Value) -> Result<String, String> {
    let query = arguments["query"]
        .as_str()
        .map(str::trim)
        .filter(|query| query.chars().count() >= 2)
        .ok_or("请提供至少两个字的搜索词，例如课程名、老师名或“选课 高数”")?;
    let limit = match &arguments["limit"] {
        Value::Null => DEFAULT_TOPICS,
        value => value
            .as_u64()
            .filter(|n| (1..=MAX_TOPICS as u64).contains(n))
            .ok_or(format!("limit 必须是 1 到 {MAX_TOPICS} 的整数"))? as usize,
    };
    let value = shuiyuan::fetch_json(&format!("/search.json?q={}", encode_query(query))).await?;
    if let Some(error) = value["grouped_search_result"]["error"].as_str().filter(|error| !error.is_empty()) {
        return Err(format!("水源搜索失败：{error}"));
    }
    let result: SearchResult = serde_json::from_value(value).map_err(|err| format!("水源搜索结果格式变化: {err}"))?;
    Ok(clip(&render_search(query, &result, limit, campus_time::now())))
}

pub(super) async fn shuiyuan_topic(arguments: Value) -> Result<String, String> {
    let id = topic_id(&arguments["topic_id"]).ok_or("请提供帖子 id（数字）或帖子链接")?;
    let start = arguments["start"].as_u64().filter(|start| *start >= 1).unwrap_or(1);
    // /t/{id}/{n}.json 返回第 n 楼附近的一页楼层
    let path = if start > 1 { format!("/t/{id}/{start}.json") } else { format!("/t/{id}.json") };
    let value = shuiyuan::fetch_json(&path).await?;
    let topic: TopicView = serde_json::from_value(value).map_err(|err| format!("水源帖子格式变化: {err}"))?;
    Ok(clip(&render_topic(&topic, start, campus_time::now())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use serde_json::json;

    fn now() -> DateTime<FixedOffset> {
        campus_time::beijing().with_ymd_and_hms(2026, 11, 25, 12, 0, 0).unwrap()
    }

    #[test]
    fn search_groups_matching_posts_under_their_topics() {
        let result: SearchResult = serde_json::from_value(json!({
            "topics": [
                {"id": 7, "title": "高等数学 选课评价汇总", "slug": "topic", "posts_count": 120, "like_count": 300, "views": 9000,
                 "created_at": "2025-06-01T02:00:00Z", "last_posted_at": "2026-11-24T08:00:00Z", "tags": ["选课", {"name": "数学"}]},
                {"id": 8, "title": "数分 vs 高数", "posts_count": 10, "tags": null}
            ],
            "posts": [
                {"username": "alice", "created_at": "2026-11-24T08:00:00Z", "like_count": 5, "blurb": "给分很好，作业&amp;小测不多", "post_number": 88, "topic_id": 7},
                {"username": "bob", "blurb": "无关帖子", "post_number": 2, "topic_id": 99}
            ]
        }))
        .unwrap();
        let text = render_search("高数", &result, 10, now());
        assert!(text.contains("找到 2 个相关帖子（显示前 2）"), "{text}");
        assert!(text.contains("1. 高等数学 选课评价汇总（帖子 id 7）"), "{text}");
        assert!(text.contains("https://shuiyuan.sjtu.edu.cn/t/topic/7"));
        assert!(text.contains("120 楼 · 300 赞 · 9000 浏览 · 发布于 2025年6月1日 周日 10:00 · 最近回复 昨天 周二 16:00 · 标签 选课、数学"), "{text}");
        assert!(text.contains("   - 88 楼 alice（昨天 周二 16:00，5 赞）：给分很好，作业&小测不多"), "{text}");
        assert!(!text.contains("无关帖子"));
        assert!(render_search("冷门", &SearchResult::default(), 10, now()).contains("没有搜到"));
    }

    #[test]
    fn topic_pages_show_floors_and_how_to_continue() {
        let topic: TopicView = serde_json::from_value(json!({
            "id": 7, "title": "高等数学 选课评价汇总", "slug": "gaoshu", "posts_count": 120, "views": 9000, "like_count": 300,
            "created_at": "2025-06-01T02:00:00Z", "tags": [],
            "post_stream": {"posts": [
                {"post_number": 2, "username": "bob", "cooked": "<p>老师讲得清楚</p>", "like_count": 3, "reply_to_post_number": 1},
                {"post_number": 1, "username": "alice", "cooked": "<p>欢迎补充</p><p>第二段</p>", "like_count": 10}
            ]}
        }))
        .unwrap();
        let text = render_topic(&topic, 1, now());
        assert!(text.contains("《高等数学 选课评价汇总》\nhttps://shuiyuan.sjtu.edu.cn/t/gaoshu/7"), "{text}");
        assert!(text.find("—— 1 楼 alice").unwrap() < text.find("—— 2 楼 bob").unwrap(), "{text}");
        assert!(text.contains("欢迎补充\n第二段"), "{text}");
        assert!(text.contains("3 赞，回复 1 楼"), "{text}");
        assert!(text.contains("已显示第 1–2 楼，共 120 楼；继续阅读请传 start=3。"), "{text}");
        assert!(render_topic(&topic, 50, now()).contains("第 50 楼之后没有更多回复"));
    }

    #[test]
    fn topic_ids_can_be_numbers_or_links() {
        assert_eq!(topic_id(&json!(351551)), Some(351551));
        assert_eq!(topic_id(&json!("351551")), Some(351551));
        assert_eq!(topic_id(&json!("https://shuiyuan.sjtu.edu.cn/t/topic/351551/20")), Some(351551));
        assert_eq!(topic_id(&json!("https://shuiyuan.sjtu.edu.cn/t/some-slug/42")), Some(42));
        assert_eq!(topic_id(&json!("水源")), None);
    }
}
