use super::clip;

#[cfg(windows)]
fn shell(command: &str) -> tokio::process::Command {
    use base64::{Engine, engine::general_purpose::STANDARD};
    // Windows PowerShell expects UTF-16LE for -EncodedCommand. This preserves
    // quotes, newlines and non-ASCII paths across the Win32 command line.
    let script = format!("[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)\n{command}");
    let bytes: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let mut process = tokio::process::Command::new("powershell.exe");
    process
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-EncodedCommand",
        ])
        .arg(STANDARD.encode(bytes));
    process
}

#[cfg(not(windows))]
fn shell(command: &str) -> tokio::process::Command {
    let mut process = tokio::process::Command::new("bash");
    process.args(["-c", command]);
    process
}

pub(super) async fn run_bash(arguments: serde_json::Value) -> Result<String, String> {
    let command = arguments["command"]
        .as_str()
        .ok_or_else(|| "bash 缺少 command 参数".to_string())?;

    // 用异步进程：阻塞等待会占住运行时线程，卡住进度事件的转发，界面就不刷新了。
    let output = shell(command)
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true)
        .output()
        .await
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
