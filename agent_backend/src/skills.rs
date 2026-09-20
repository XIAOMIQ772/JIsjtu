use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

pub struct Skill {
    pub name: String,
    pub description: String,
    pub path: PathBuf,
}

static SKILLS: OnceLock<Vec<Skill>> = OnceLock::new();

/// 技能目录：优先取 AGENT_SKILLS_DIR，否则用当前目录下的 skills/，
/// 再退回到上一级的 skills/（在 agent_backend 里 cargo run 时命中）。
pub fn dir() -> PathBuf {
    if let Ok(dir) = std::env::var("AGENT_SKILLS_DIR") {
        return PathBuf::from(dir);
    }
    if PathBuf::from("skills").is_dir() {
        return PathBuf::from("skills");
    }
    PathBuf::from("../skills")
}

pub fn all() -> &'static [Skill] {
    SKILLS.get_or_init(|| load(&dir()))
}

pub fn load(dir: &Path) -> Vec<Skill> {
    let mut skills = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return skills;
    };

    for entry in entries.flatten() {
        if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        let path = entry.path().join("SKILL.md");
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        let fallback = entry.file_name().to_string_lossy().into_owned();
        let (name, description) = parse(&text, &fallback);
        skills.push(Skill {
            name,
            description,
            path: fs::canonicalize(&path).unwrap_or(path),
        });
    }

    skills.sort_by(|left, right| left.name.cmp(&right.name));
    skills
}

pub fn prompt_section() -> String {
    render(all())
}

fn render(skills: &[Skill]) -> String {
    if skills.is_empty() {
        return String::new();
    }

    let mut text = String::from(
        "
                *可用技能*
                下面是本机技能目录里的技能说明。每个技能是一个 Markdown 文件，写明了某类任务的完整流程、参数和注意事项。
                当用户的请求与某个技能的描述相符时，必须先用 read 工具读取该技能文件的全文，再严格按其中的步骤执行，不要凭记忆猜测流程。
                技能列表：
",
    );
    for skill in skills {
        text.push_str(&format!(
            "                - {}：{}\n                  文件：{}\n",
            skill.name,
            if skill.description.is_empty() {
                "（无描述）"
            } else {
                skill.description.as_str()
            },
            skill.path.display()
        ));
    }
    text
}

/// 解析 SKILL.md 开头的 YAML 前言，只取 name 和 description 两个字段。
/// 没有前言时用目录名当 name、正文第一行当 description。
fn parse(text: &str, fallback_name: &str) -> (String, String) {
    let lines: Vec<&str> = text.lines().collect();
    let mut name = fallback_name.to_string();
    let mut description = String::new();
    let mut body_at = 0;

    if lines.first().is_some_and(|line| line.trim() == "---") {
        let closing = lines[1..].iter().position(|line| line.trim() == "---");
        let (fields, body) = match closing {
            Some(offset) => (&lines[1..=offset], offset + 2),
            None => (&lines[1..], lines.len()),
        };

        for line in fields {
            let Some((key, value)) = line.split_once(':') else {
                continue;
            };
            let value = value.trim().trim_matches(['"', '\'']);
            match key.trim() {
                "name" if !value.is_empty() => name = value.to_string(),
                "description" => description = value.to_string(),
                _ => {}
            }
        }
        body_at = body;
    }

    if description.is_empty() {
        description = lines
            .get(body_at..)
            .unwrap_or_default()
            .iter()
            .map(|line| line.trim().trim_start_matches('#').trim())
            .find(|line| !line.is_empty())
            .map(|line| (*line).to_string())
            .unwrap_or_default();
    }

    (name, description)
}


