use super::clip;

pub(super) async fn run_bash(arguments: serde_json::Value) -> Result<String, String> {
    let command = arguments["command"]
        .as_str()
        .ok_or_else(|| "bash 缺少 command 参数".to_string())?;

    let output = std::process::Command::new("bash")
        .arg("-c")
        .arg(command)
        .output()
        .map_err(|err| err.to_string())?;

    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    if !output.status.success() {
        text.push_str(&format!(
            "\n[退出码: {}]",
            output.status.code().unwrap_or(-1)
        ));
    }

    Ok(clip(&text))
}
