//! 水源社区（Discourse）共用部分：在同一个后台浏览器页面里批量请求 JSON 接口。
//!
//! 水源的 JSON 接口未登录时直接返回 403，不会跳转登录；先打开首页让浏览器
//! 走完 jAccount 授权，再在页面内请求接口，就能用上登录态，且多个接口只启动一次浏览器。

use serde_json::Value;

use crate::sso::{FetchRequest, campus_fetch};

pub(super) const SHUIYUAN: &str = "https://shuiyuan.sjtu.edu.cn";
const JSON_LIMIT: usize = 1_500_000;

fn decode(response: &crate::sso::FetchResponse) -> Result<Value, String> {
    if let Some(error) = &response.error {
        return Err(format!("请求水源失败：{error}"));
    }
    match response.status {
        200 => {}
        403 => return Err("水源拒绝访问（403）：帖子可能在受限分区，或登录状态失效".into()),
        404 => return Err("水源上找不到该内容（404），可能已删除或无权查看".into()),
        429 => return Err("水源请求太频繁（429），请稍后再试".into()),
        status => return Err(format!("水源返回 {status}")),
    }
    if response.truncated {
        return Err("水源返回的内容过长，已被截断".into());
    }
    serde_json::from_str(&response.text).map_err(|_| "水源返回的不是 JSON，可能登录失效，请稍后重试".to_string())
}

/// 一次后台浏览器会话里请求多个水源 JSON 接口，按顺序返回各自的结果。
pub(super) async fn fetch_json_many(paths: &[String]) -> Result<Vec<Result<Value, String>>, String> {
    let requests: Vec<FetchRequest> = paths.iter().map(FetchRequest::get).collect();
    let responses = campus_fetch(&format!("{SHUIYUAN}/"), &requests, JSON_LIMIT).await?;
    Ok(responses.iter().map(decode).collect())
}

pub(super) async fn fetch_json(path: &str) -> Result<Value, String> {
    fetch_json_many(&[path.to_string()]).await?.remove(0)
}

pub(super) fn topic_url(id: u64, slug: &str) -> String {
    format!("{SHUIYUAN}/t/{}/{id}", if slug.is_empty() { "topic" } else { slug })
}

/// 查询串的百分号编码（Discourse 搜索词里常有中文、空格、冒号、#）。
pub(super) fn encode_query(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(byte as char),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sso::FetchResponse;

    #[test]
    fn responses_are_decoded_with_readable_errors() {
        let ok = FetchResponse { status: 200, text: r#"{"a":1}"#.into(), ..Default::default() };
        assert_eq!(decode(&ok).unwrap()["a"], 1);
        let html = FetchResponse { status: 200, text: "<html>".into(), ..Default::default() };
        assert!(decode(&html).unwrap_err().contains("不是 JSON"));
        let denied = FetchResponse { status: 403, ..Default::default() };
        assert!(decode(&denied).unwrap_err().contains("403"));
        let failed = FetchResponse { error: Some("TypeError".into()), ..Default::default() };
        assert!(decode(&failed).unwrap_err().contains("TypeError"));
        assert_eq!(encode_query("高数 order:latest"), "%E9%AB%98%E6%95%B0%20order%3Alatest");
        assert_eq!(topic_url(5, ""), "https://shuiyuan.sjtu.edu.cn/t/topic/5");
    }
}
