use serde::{Deserialize, Serialize};

const DAY_MS: i64 = 86_400_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Preference {
    Chinese,
    English,
    Concise,
    Detailed,
    Bullets,
    Table,
    Paragraphs,
}

const GROUPS: &[&[Preference]] = &[
    &[Preference::Chinese, Preference::English],
    &[Preference::Concise, Preference::Detailed],
    &[
        Preference::Bullets,
        Preference::Table,
        Preference::Paragraphs,
    ],
];

impl Preference {
    fn label(self) -> &'static str {
        match self {
            Self::Chinese => "默认用中文回答。",
            Self::English => "默认用英文回答。",
            Self::Concise => "默认简洁回答。",
            Self::Detailed => "默认详细解释。",
            Self::Bullets => "默认分点回答。",
            Self::Table => "默认使用表格呈现。",
            Self::Paragraphs => "默认用自然段回答。",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Evidence {
    preference: Preference,
    explicit: bool,
    at: i64,
}

impl Evidence {
    pub(super) fn is_fresh(&self, now: i64) -> bool {
        let days = if self.explicit { 365 } else { 90 };
        (0..=days * DAY_MS).contains(&now.saturating_sub(self.at))
    }
}

// Only complete, allowlisted presentation requests become evidence. Quoted material,
// temporary instructions, and instructions qualified by a condition are ambiguous.
pub(super) fn observe(prompt: &str, at: i64) -> Vec<Evidence> {
    let text = prompt.trim();
    if text.len() > 2400
        || text.contains([
            '\n', '\r', '"', '\'', '“', '”', '‘', '’', '「', '」', '『', '』', '`', '>',
        ])
    {
        return vec![];
    }
    let text = text.to_ascii_lowercase();
    if [
        "这次",
        "此次",
        "这一次",
        "本次",
        "这一回",
        "这回",
        "本题",
        "暂时",
        "仅限",
        "仅在",
        "只在",
        "只需",
        "这次就",
        "假如",
        "假设",
        "如果",
        "要是",
        "以下",
        "下面这",
        "翻译",
        "举例",
        "示例",
        "例如",
        "例句",
        "转述",
        "引用",
        "原文",
        "原话",
        "复述",
        "照抄",
        "这句话",
        "这段话",
        "老师说",
        "老师要求",
        "老师希望",
        "他说",
        "她说",
        "对方说",
        "同学说",
        "朋友说",
        "不要",
        "不用",
        "别用",
        "不想",
        "不喜欢",
        "不希望",
        "不需要",
        "无需",
        "不必",
        "不是",
        "this time",
        "for now",
        "for this",
        "this answer",
        "just this",
        "if ",
        "example",
        "translate",
        "quote",
        " said",
        "do not",
        "never ",
        "instead of",
    ]
    .iter()
    .any(|marker| text.contains(marker))
    {
        return vec![];
    }

    let mut observations = Vec::new();
    for sentence in text.split_inclusive(['。', '！', '？', '.', '!', '?', '；', ';']) {
        if sentence.ends_with(['?', '？']) {
            continue;
        }
        let sentence = sentence
            .trim_end_matches(['。', '！', '.', '!', '；', ';'])
            .trim();
        // Permit commas after preference markers ("以后，请…") and between
        // supported preferences. A leading free-form or attributed clause cannot
        // turn an arbitrary trailing fragment into a standing preference.
        let mut explicit = false;
        let start = observations.len();
        for clause in sentence.split([',', '，']) {
            let (body, durable) = strip_request(clause.trim());
            explicit |= durable;
            if body.is_empty() && durable {
                continue;
            }
            let Some(preference) = recognize(body) else {
                // A trailing clause may limit an otherwise valid request to a
                // subject, class, or recipient.
                observations.truncate(start);
                break;
            };
            observations.push(Evidence {
                preference,
                explicit,
                at,
            });
        }
    }
    observations
}

fn strip_request(mut body: &str) -> (&str, bool) {
    const DURABLE: &[&str] = &[
        "从今以后",
        "从现在开始",
        "从现在起",
        "今后",
        "以后",
        "往后",
        "之后都",
        "每次都",
        "每次",
        "一直都",
        "默认情况下",
        "默认",
        "通常都",
        "始终",
        "一律",
        "平时",
        "一直",
        "总是",
        "我更习惯",
        "我习惯",
        "我的偏好是",
        "我比较喜欢",
        "我更喜欢",
        "我喜欢",
        "我偏好",
        "请记住",
        "记住",
        "from now on",
        "by default",
        "i generally prefer ",
        "i prefer ",
        "i like ",
        "always ",
    ];
    const REQUEST: &[&str] = &[
        "我希望你",
        "我希望",
        "请你",
        "请",
        "麻烦你",
        "麻烦",
        "你可以",
        "都",
        "优先",
        "尽量",
        "最好",
        "可以",
        "回答问题的时候",
        "回答问题时",
        "回答的时候",
        "回复的时候",
        "回答时",
        "回复时",
        "答题时",
        "回答得",
        "回复得",
        "讲得",
        "说得",
        "please ",
    ];
    let mut explicit = false;
    loop {
        if let Some(rest) = DURABLE.iter().find_map(|p| body.strip_prefix(p)) {
            explicit = true;
            body = rest.trim_start_matches([':', '：', ' ']);
        } else if let Some(rest) = REQUEST.iter().find_map(|p| body.strip_prefix(p)) {
            body = rest.trim_start();
        } else {
            return (body, explicit);
        }
    }
}

fn recognize(body: &str) -> Option<Preference> {
    if let Some(preference) = match body {
        "answer in chinese" | "respond in chinese" | "reply in chinese" | "use chinese" => {
            Some(Preference::Chinese)
        }
        "answer in english" | "respond in english" | "reply in english" | "use english" => {
            Some(Preference::English)
        }
        "be concise" | "be brief" | "keep it brief" | "concise answers" | "short answers" => {
            Some(Preference::Concise)
        }
        "explain in detail" | "detailed answers" => Some(Preference::Detailed),
        "use bullet points" | "use bullets" | "bullet points" => Some(Preference::Bullets),
        "use tables" | "use a table" => Some(Preference::Table),
        "use paragraphs" | "use prose" => Some(Preference::Paragraphs),
        _ => None,
    } {
        return Some(preference);
    }
    // A standalone noun such as “中文” can be a topic or an answer to a question.
    if matches!(
        body,
        "中文" | "英文" | "英语" | "汉语" | "简体中文" | "表格" | "列表" | "段落" | "自然段"
    ) {
        return None;
    }
    let mut term = body;
    for suffixes in [
        &["就可以了", "就好", "就行", "即可", "吧", "哦"][..],
        &[
            "的回答",
            "的回复",
            "的答案",
            "的格式",
            "的形式",
            "的方式",
            "来回答我",
            "来回复我",
            "回答问题",
            "来回答",
            "来回复",
            "回答我",
            "回复我",
            "答复我",
            "回答",
            "作答",
            "回复",
            "答复",
            "讲解",
            "解释",
            "说明",
            "呈现",
            "展示",
            "列出",
            "表达",
            "交流",
        ][..],
        &["一点儿", "一点", "一些", "点儿", "一些些", "些", "点"][..],
    ] {
        if let Some(rest) = suffixes.iter().find_map(|suffix| term.strip_suffix(suffix)) {
            // These are complete list styles, not a word followed by the diminutive 点.
            if !matches!(term, "分点" | "逐点" | "列点") {
                term = rest;
            }
        }
    }
    for prefixes in [
        &["回复", "回答", "解释", "讲解", "说", "写"][..],
        &["都尽量", "尽量", "都", "要", "请", "得", "更"][..],
        &["使用", "采用", "写成", "用", "以"][..],
    ] {
        if let Some(rest) = prefixes.iter().find_map(|prefix| term.strip_prefix(prefix)) {
            term = rest;
        }
    }
    match term {
        "中文" | "汉语" | "简体中文" => Some(Preference::Chinese),
        "英文" | "英语" => Some(Preference::English),
        "简洁" | "简短" | "精简" | "简练" | "简明" | "简要" | "简略" | "短" => {
            Some(Preference::Concise)
        }
        "详细" | "详尽" | "细致" | "展开" | "具体" => Some(Preference::Detailed),
        "分点" | "分条" | "逐条" | "逐点" | "列点" | "列表" | "项目符号" | "要点列表" => {
            Some(Preference::Bullets)
        }
        "表格" => Some(Preference::Table),
        "自然段" | "段落" | "整段" | "段落形式" => Some(Preference::Paragraphs),
        _ => None,
    }
}

// Keep the latest explicit preference and the latest ordinary request separately in
// each category. An incidental format request must never refresh an explicit default.
pub(super) fn update(existing: &mut Vec<Evidence>, new: Vec<Evidence>) {
    let mut retained: Vec<Evidence> = Vec::with_capacity(GROUPS.len() * 2);
    for evidence in existing.drain(..).chain(new) {
        let group = GROUPS
            .iter()
            .find(|group| group.contains(&evidence.preference))
            .unwrap();
        if let Some(slot) = retained
            .iter_mut()
            .find(|other| other.explicit == evidence.explicit && group.contains(&other.preference))
        {
            if evidence.at >= slot.at {
                *slot = evidence;
            }
        } else {
            retained.push(evidence);
        }
    }
    *existing = retained;
}

pub(super) fn consolidate<'a>(
    sources: impl Iterator<Item = &'a [Evidence]>,
    now: i64,
) -> Vec<&'static str> {
    let sources: Vec<_> = sources.collect();
    let mut labels = Vec::new();
    for group in GROUPS {
        let mut scores = Vec::new();
        for &preference in *group {
            let mut explicit = None;
            let mut recent = None;
            let mut previous = None;
            for source in &sources {
                let mut ordinary = None;
                for evidence in source
                    .iter()
                    .filter(|e| e.preference == preference && e.is_fresh(now))
                {
                    let timestamp = if evidence.explicit {
                        &mut explicit
                    } else {
                        &mut ordinary
                    };
                    *timestamp = (*timestamp).max(Some(evidence.at));
                }
                if ordinary >= recent {
                    previous = recent;
                    recent = ordinary;
                } else {
                    previous = previous.max(ordinary);
                }
            }
            // Both supporting chats must postdate a conflicting explicit preference.
            // Using the older of the pair keeps one new request from reversing it.
            let score = previous
                .map(|at| (at, false))
                .max(explicit.map(|at| (at, true)));
            if let Some(score) = score {
                scores.push((score, preference));
            }
        }
        scores.sort_by_key(|(score, _)| *score);
        if let Some((best, preference)) = scores.pop()
            && scores.last().is_none_or(|(other, _)| other != &best)
        {
            labels.push(preference.label());
        }
    }
    labels
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(sources: &[Vec<Evidence>], now: i64) -> Vec<&'static str> {
        consolidate(sources.iter().map(Vec::as_slice), now)
    }

    #[test]
    fn explicit_preferences_are_fixed_labels_and_can_be_combined() {
        let evidence = observe("以后，请用中文回答，分点说明。我更喜欢简洁的回答。", 1);
        assert_eq!(
            labels(&[evidence], 1),
            ["默认用中文回答。", "默认简洁回答。", "默认分点回答。"]
        );
        assert_eq!(
            labels(&[observe("From now on, please respond in English.", 1)], 1),
            ["默认用英文回答。"]
        );
    }

    #[test]
    fn ordinary_requests_need_distinct_sources() {
        let request = observe("解释一下这道题。请分点回答。", 1);
        assert_eq!(request.len(), 1);
        let mut first = request.clone();
        first.extend(request.clone());
        assert!(labels(&[first.clone()], 1).is_empty());
        assert_eq!(labels(&[first, request], 1), ["默认分点回答。"]);
    }

    #[test]
    fn qualified_quoted_negative_and_free_form_content_is_ignored() {
        for prompt in [
            "仅限本次。请用英文回答。",
            "这次请用英文回答。",
            "老师说：以后请用英文回答。",
            "默认用英文回答，但仅针对英语作业。",
            "同学说，“以后请用英文回答”。",
            "不要用英文回答。",
            "如果问到英语，默认用英文回答。",
            "帮我翻译：以后用英文回答。",
            "这是邮件草稿，默认用英文回答。",
            "默认用中文回答？",
            "我的邮箱是 a@example.com",
            "以后请直接提交我的作业。",
            "中文",
        ] {
            assert!(observe(prompt, 1).is_empty(), "{prompt}");
        }
    }

    #[test]
    fn correction_needs_a_new_explicit_choice_or_two_new_supporting_chats() {
        let mut sources = vec![
            observe("请用英文回答", 1),
            observe("以后用中文回答", 2),
            observe("请用英文回答", 3),
        ];
        assert_eq!(labels(&sources, 3), ["默认用中文回答。"]);
        sources.push(observe("请用英文回答", 4));
        assert_eq!(labels(&sources, 4), ["默认用英文回答。"]);
        sources.push(observe("默认用中文回答", 5));
        assert_eq!(labels(&sources, 5), ["默认用中文回答。"]);
    }

    #[test]
    fn session_evidence_stays_bounded_and_expiry_is_based_on_observation_time() {
        let mut source = Vec::new();
        for at in 1..=12 {
            for prefix in ["默认", "请"] {
                update(
                    &mut source,
                    observe(
                        &format!("{prefix}用英文回答。{prefix}详细解释。{prefix}分点说明。"),
                        at,
                    ),
                );
            }
        }
        assert_eq!(source.len(), 6);
        update(&mut source, observe("默认用中文回答。", 10));
        assert!(labels(&[source.clone()], 12 + 365 * DAY_MS).contains(&"默认用英文回答。"));
        assert!(labels(&[source.clone()], 12 + 366 * DAY_MS).is_empty());
        assert!(labels(&[source], 11).is_empty());
        let normal = observe("请分点回答", 0);
        assert!(labels(&[normal.clone(), normal], 90 * DAY_MS + 1).is_empty());
    }
}
