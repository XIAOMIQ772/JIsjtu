//! library_status：交大各图书馆实时在馆人数（图书馆公开的人数统计接口，无需登录）。

use chrono::TimeZone;

use super::campus_time::{self, fmt_time};

const LIBRARY_COUNT_URL: &str = "https://zgrstj.lib.sjtu.edu.cn/cp";

#[derive(serde::Deserialize)]
struct Counts {
    #[serde(default)]
    timestamp: Option<i64>,
    #[serde(default)]
    numbers: Vec<Area>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct Area {
    #[serde(default)]
    area_name: String,
    #[serde(default)]
    in_counter: i64,
    #[serde(default)]
    max: i64,
}

/// 接口返回 JSONP：CountPerson({...});
fn parse_jsonp(body: &str) -> Result<Counts, String> {
    let start = body.find('(').map_or(0, |index| index + 1);
    let end = body.rfind(')').filter(|end| *end >= start).unwrap_or(body.len());
    serde_json::from_str(body[start..end].trim()).map_err(|err| format!("图书馆人数数据格式变化，解析失败: {err}"))
}

fn render(counts: &Counts) -> String {
    let now = campus_time::now();
    let updated = counts
        .timestamp
        .and_then(|millis| campus_time::beijing().timestamp_millis_opt(millis).single())
        .map(|time| format!("（{} 更新）", fmt_time(time, now)))
        .unwrap_or_default();
    let mut text = format!("交大图书馆实时在馆人数{updated}\n");
    let mut quietest: Option<(&Area, f64)> = None;
    for area in &counts.numbers {
        let ratio = if area.max > 0 { area.in_counter as f64 / area.max as f64 } else { 0.0 };
        text.push_str(&format!(
            "- {}：{} 人 / 容量 {}（{:.0}%）\n",
            area.area_name.trim(),
            area.in_counter,
            area.max,
            ratio * 100.0
        ));
        if area.max > 0 && quietest.is_none_or(|(_, best)| ratio < best) {
            quietest = Some((area, ratio));
        }
    }
    if let Some((area, _)) = quietest {
        text.push_str(&format!("按占用比例，现在最空的是{}。\n", area.area_name.trim()));
    }
    text.push_str("人数来自图书馆闸机统计；开放时间与座位预约以图书馆官网（lib.sjtu.edu.cn）为准。");
    text
}

pub(super) async fn library_status(_: serde_json::Value) -> Result<String, String> {
    let body = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|err| err.to_string())?
        .get(LIBRARY_COUNT_URL)
        .send()
        .await
        .and_then(|response| response.error_for_status())
        .map_err(|err| format!("读取图书馆人数失败: {err}"))?
        .text()
        .await
        .map_err(|err| format!("读取图书馆人数失败: {err}"))?;
    let counts = parse_jsonp(&body)?;
    if counts.numbers.is_empty() {
        return Err("图书馆人数接口没有返回任何场馆".into());
    }
    Ok(render(&counts))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jsonp_counts_are_parsed_and_quietest_library_is_named() {
        let body = r#"CountPerson({"timestamp":1795618867743,"numbers":[{"areaName":"图书馆主馆","inCounter":680,"max":5000},{"areaName":"李政道图书馆","inCounter":470,"max":1000},{"areaName":"包玉刚图书馆","inCounter":131,"max":4000}]});"#;
        let counts = parse_jsonp(body).unwrap();
        assert_eq!(counts.numbers.len(), 3);
        let text = render(&counts);
        assert!(text.contains("- 图书馆主馆：680 人 / 容量 5000（14%）"), "{text}");
        assert!(text.contains("- 李政道图书馆：470 人 / 容量 1000（47%）"), "{text}");
        assert!(text.contains("现在最空的是包玉刚图书馆"), "{text}");
        assert!(parse_jsonp("<html>维护中</html>").is_err());
    }
}
