use super::page_is_for_user;

pub(super) async fn watch_eduinfo(arguments: serde_json::Value)->Result<String,String>{
    let base_url="https://i.sjtu.edu.cn/xtgl/login_slogin.html";
    crate::sso::visit_site(base_url, page_is_for_user(&arguments, false)?).await
}
