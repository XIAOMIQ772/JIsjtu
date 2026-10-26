use super::page_is_for_user;

pub(super) async fn open_usual_website(arguments: serde_json::Value)->Result<String,String>{
    let show = page_is_for_user(&arguments, false)?;
    let base_url = arguments["url"].as_str().filter(|url| *url != "none")
        .ok_or_else(|| "请提供有效的网页 URL".to_string())?;
    crate::sso::visit_site(base_url, show).await
}
