use super::page_is_for_user;

pub(super) async fn watch_shuiyuan(arguments: serde_json::Value)->Result<String,String>{
    let base_url="https://shuiyuan.sjtu.edu.cn";
    crate::sso::visit_site(base_url, page_is_for_user(&arguments, true)?).await
}
