use super::clip;

pub(super) async fn run_read(arguments: serde_json::Value) -> Result<String, String> {
    let path = arguments["path"]
        .as_str()
        .ok_or_else(|| "read 缺少 path 参数".to_string())?;
    let offset = arguments["offset"].as_u64().unwrap_or(1).max(1) as usize;
    let limit = arguments["limit"].as_u64().map(|value| value as usize);

    let meta = std::fs::metadata(path).map_err(|err| err.to_string())?;
    if meta.is_dir() {
        let mut names = Vec::new();
        for entry in std::fs::read_dir(path).map_err(|err| err.to_string())? {
            let entry = entry.map_err(|err| err.to_string())?;
            let mut name = entry.file_name().to_string_lossy().into_owned();
            if entry.file_type().map_err(|err| err.to_string())?.is_dir() {
                name.push('/');
            }
            names.push(name);
        }
        names.sort();
        return Ok(clip(&names.join("\n")));
    }

    let content = std::fs::read_to_string(path).map_err(|err| err.to_string())?;
    let lines: Vec<&str> = content.lines().collect();
    let start = offset - 1;
    if start >= lines.len() {
        return Ok(format!(
            "文件共 {} 行，offset {offset} 已超出范围",
            lines.len()
        ));
    }
    let end = match limit {
        Some(limit) => (start + limit).min(lines.len()),
        None => lines.len(),
    };

    let mut text = String::new();
    for (index, line) in lines[start..end].iter().enumerate() {
        text.push_str(&format!("{}: {line}\n", start + index + 1));
    }
    Ok(clip(&text))
}
