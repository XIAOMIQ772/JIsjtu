pub(super) async fn run_edit(arguments: serde_json::Value) -> Result<String, String> {
    let path = arguments["path"]
        .as_str()
        .ok_or_else(|| "edit 缺少 path 参数".to_string())?;
    let old_context = arguments["old_context"]
        .as_str()
        .ok_or_else(|| "edit 缺少 old_context 参数".to_string())?;
    let new_context = arguments["new_context"]
        .as_str()
        .ok_or_else(|| "edit 缺少 new_context 参数".to_string())?;

    let content = std::fs::read_to_string(path).map_err(|err| err.to_string())?;
    match content.matches(old_context).count() {
        0 => return Err(format!("old_context 在 {path} 中未找到")),
        1 => {}
        count => return Err(format!("old_context 在 {path} 中匹配到 {count} 处，需唯一")),
    }

    let updated = content.replacen(old_context, new_context, 1);
    std::fs::write(path, updated).map_err(|err| err.to_string())?;
    Ok(format!("已替换 {path} 中的 1 处内容"))
}
