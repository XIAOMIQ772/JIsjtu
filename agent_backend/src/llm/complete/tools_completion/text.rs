//! 文本处理共用函数：HTML 转纯文本、截断、压成一行。

pub(super) fn decode_entity(entity: &str) -> Option<String> {
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
pub(super) fn html_to_text(html: &str) -> String {
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

        let after = &rest[1..];
        if !after.trim_start_matches('/').starts_with(|c: char| c.is_ascii_alphabetic() || matches!(c, '!' | '?')) {
            out.push('<');
            rest = after;
            continue;
        }
        let mut quoted = None;
        let end = after.char_indices().find_map(|(index, ch)| match (quoted, ch) {
            (None, '>') => Some(index),
            (None, '"' | '\'') => { quoted = Some(ch); None }
            (Some(q), _) if q == ch => { quoted = None; None }
            _ => None,
        });
        let Some(end) = end else { out.push_str(rest); rest = ""; break };
        let tag = after[..end].trim();
        let attrs = tag.split_once(char::is_whitespace).map_or("", |(_, attrs)| attrs);
        rest = &after[end + 1..];
        let closing = tag.starts_with('/');
        let name: String = tag
            .trim_start_matches('/')
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect::<String>().to_ascii_lowercase();

        match (name.as_str(), closing) {
            ("ol", false) => lists.push(Some(super::office::attr(attrs, "start")
                .and_then(|v| v.parse::<u32>().ok()).unwrap_or(1).saturating_sub(1))),
            ("ul", false) => lists.push(None),
            ("ol" | "ul", true) => {
                lists.pop();
                out.push('\n');
            }
            ("li", false) => {
                out.push('\n');
                match lists.last_mut() {
                    Some(Some(counter)) => {
                        *counter = super::office::attr(attrs, "value").and_then(|v| v.parse().ok())
                            .unwrap_or_else(|| counter.saturating_add(1));
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
            ("img", false) => {
                let attr = |key| super::office::attr(attrs, key).filter(|s| !s.trim().is_empty());
                if let Some(equation) = attr("data-equation-content") {
                    out.push_str(&equation);
                } else if let Some(alt) = attr("alt") {
                    out.push_str(&format!("[图片：{alt}]"));
                } else {
                    out.push_str("[图片]");
                }
            }
            ("sup", false) => out.push_str("^("),
            ("sub", false) => out.push_str("_("),
            ("sup" | "sub", true) => out.push(')'),
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

/// 超过 max_chars 个字符时截断并加省略号。
pub(super) fn shorten(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        text.to_string()
    } else {
        format!("{}…", text.chars().take(max_chars).collect::<String>())
    }
}

/// 把多行、多空白压成一行，用于列表里的摘要。
pub(super) fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shorten_and_one_line_count_characters_not_bytes() {
        assert_eq!(shorten("交我集小助手", 3), "交我集…");
        assert_eq!(shorten("短", 3), "短");
        assert_eq!(one_line(" a\n\n b\tc "), "a b c");
        assert_eq!(html_to_text("<p>x&nbsp;&lt;y</p><img src=a>"), "x <y\n[图片]");
    }
}
