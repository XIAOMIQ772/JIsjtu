//! 工具实现：每个工具一个子模块，这里只负责按名字分发和少量共用的辅助函数。

mod bash;
mod campus_time;
mod canvas;
mod course_files;
mod create_subagents;
mod daily_brief;
mod degree_progress;
mod edit;
mod eduinfo;
mod export_calendar;
mod find_classroom;
mod get_announcements;
mod get_courses;
mod get_deadlines;
mod get_exam;
mod get_grades;
mod get_hotnews;
mod get_time_stamp;
mod homework;
mod library_status;
mod mail_fetch;
mod office;
mod open_usual_website;
mod pdf_analysis;
mod read;
mod shuiyuan;
mod shuiyuan_search;
mod text;
mod watch_eduinfo;
mod watch_shuiyuan;
mod write;

pub use homework::USER_NOTICE_MARK;

const MAX_OUTPUT_CHARS: usize = 16000;

pub async fn call(name: &str, arguments: serde_json::Value) -> Result<String, String> {
    match name {
        "sso_login" => crate::sso::login(arguments).await,
        "bash" => bash::run_bash(arguments).await,
        "read" => read::run_read(arguments).await,
        "write" => write::run_write(arguments).await,
        "edit" => edit::run_edit(arguments).await,
        "get_time_stamp"=>get_time_stamp::get_time_stamp(arguments).await,
        "daily_brief"=>daily_brief::daily_brief(arguments).await,
        "get_courses"=>get_courses::get_courses(arguments).await,
        "get_exam"=>get_exam::get_exam(arguments).await,
        "get_deadlines"=>get_deadlines::get_deadlines(arguments).await,
        "get_announcements"=>get_announcements::get_announcements(arguments).await,
        "get_grades"=>get_grades::get_grades(arguments).await,
        "find_classroom"=>find_classroom::find_classroom(arguments).await,
        "library_status"=>library_status::library_status(arguments).await,
        "get_hotnews"=>get_hotnews::get_hotnews(arguments).await,
        "watch_shuiyuan"=>watch_shuiyuan::watch_shuiyuan(arguments).await,
        "shuiyuan_search"=>shuiyuan_search::shuiyuan_search(arguments).await,
        "shuiyuan_topic"=>shuiyuan_search::shuiyuan_topic(arguments).await,
        "pdf_analysis"=>pdf_analysis::pdf_analysis(arguments).await,
        "read_office"=>office::read_office(arguments).await,
        "course_files"=>course_files::course_files(arguments).await,
        "watch_eduinfo"=>watch_eduinfo::watch_eduinfo(arguments).await,
        "class_schedule"=>eduinfo::class_schedule(arguments).await,
        "exam_schedule"=>eduinfo::exam_schedule(arguments).await,
        "score_report"=>eduinfo::score_report(arguments).await,
        "export_calendar"=>export_calendar::export_calendar(arguments).await,
        "degree_progress"=>degree_progress::degree_progress(arguments).await,
        "mail_fetch"=>mail_fetch::mail_fetch(arguments).await,
        "open_usual_website"=>open_usual_website::open_usual_website(arguments).await,
        "create_subagents"=>create_subagents::create_subagents(arguments).await,
        "homework"=>homework::homework(arguments).await,
        _ => Err("tool not found ,check tool name".to_string()),
    }
}

fn clip(text: &str) -> String {
    if text.len() < MAX_OUTPUT_CHARS {
        text.to_string()
    } else {
        let end = text.floor_char_boundary(MAX_OUTPUT_CHARS);
        format!("{}\n<output clipped>", &text[..end])
    }
}

fn page_is_for_user(arguments: &serde_json::Value, default_show: bool) -> Result<bool, String> {
    if !arguments.is_object() {
        return Err("网页工具参数必须是对象".into());
    }
    match arguments.get("mode") {
        None | Some(serde_json::Value::Null) => Ok(default_show),
        Some(serde_json::Value::String(mode)) if mode == "read" => Ok(false),
        Some(serde_json::Value::String(mode)) if mode == "show" => Ok(true),
        _ => Err("mode 必须为 read 或 show；查询任务使用 read".into()),
    }
}

#[cfg(test)]
mod tests;
