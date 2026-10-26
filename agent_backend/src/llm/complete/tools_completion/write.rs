pub(super) async fn run_write(arguments: serde_json::Value) -> Result<String, String> {
    let path = arguments["path"]
        .as_str()
        .ok_or_else(|| "write 缺少 path 参数".to_string())?;
    let content = arguments["content"]
        .as_str()
        .ok_or_else(|| "write 缺少 content 参数".to_string())?;

    if let Some(parent) = std::path::Path::new(path).parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|err| err.to_string())?;
        }
    }

    std::fs::write(path, content).map_err(|err| err.to_string())?;
    Ok(format!("已写入 {path}，{} 字节", content.len()))
}
