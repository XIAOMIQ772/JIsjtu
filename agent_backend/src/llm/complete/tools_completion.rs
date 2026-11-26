//! 工具实现：每个工具一个子模块，这里只负责按名字分发和少量共用的辅助函数。

#[cfg(any(target_os = "macos", windows))]
mod attendance_class;
#[cfg(any(target_os = "macos", windows))]
mod attendance_source;
#[cfg(any(target_os = "macos", windows))]
pub(crate) mod attendance_watch;
mod bash;
mod campus_time;
mod canvas;
mod course_files;
mod course_reviews;
mod create_subagents;
mod daily_brief;
mod degree_progress;
mod downloads;
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
pub(crate) mod outbound;
mod open_usual_website;
mod pdf_analysis;
mod pdf_page_view;
#[cfg(windows)]
mod pdf_windows;
mod read;
#[cfg(any(target_os = "macos", windows))]
pub(crate) mod reminders;
mod search_files;
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
        "course_reviews"=>course_reviews::course_reviews(arguments).await,
        "search_files"=>search_files::search_files(arguments).await,
        "watch_eduinfo"=>watch_eduinfo::watch_eduinfo(arguments).await,
        "class_schedule"=>eduinfo::class_schedule(arguments).await,
        "watch_attendance"=>watch_attendance(arguments).await,
        "exam_schedule"=>eduinfo::exam_schedule(arguments).await,
        "score_report"=>eduinfo::score_report(arguments).await,
        "export_calendar"=>export_calendar::export_calendar(arguments).await,
        "degree_progress"=>degree_progress::degree_progress(arguments).await,
        "mail_fetch"=>mail_fetch::mail_fetch(arguments).await,
        "send_email"=>outbound::send_email(arguments).await,
        "shuiyuan_post"=>outbound::shuiyuan_post(arguments).await,
        "open_usual_website"=>open_usual_website::open_usual_website(arguments).await,
        "create_subagents"=>create_subagents::create_subagents(arguments).await,
        "homework"=>homework::homework(arguments).await,
        _ => Err("tool not found ,check tool name".to_string()),
    }
}

#[cfg(any(target_os = "macos", windows))]
async fn watch_attendance(arguments: serde_json::Value) -> Result<String, String> {
    if !attendance_watch::is_running() {
        return Err("请启动 JIsjtu，在终端聊天或网页对话里安排签到提醒。".into());
    }
    let class = attendance_class::resolve(&arguments).await?;
    attendance_watch::schedule(class.course_id, &class.course_name, class.start)
}

#[cfg(not(any(target_os = "macos", windows)))]
async fn watch_attendance(_: serde_json::Value) -> Result<String, String> {
    Err("课堂签到监看需要在 Windows 或 macOS 版 JIsjtu 中运行。".into())
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
