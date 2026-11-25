use super::campus_time;

pub(super) async fn get_time_stamp(_: serde_json::Value) -> Result<String, String> {
    Ok(campus_time::describe_now(campus_time::now()))
}
