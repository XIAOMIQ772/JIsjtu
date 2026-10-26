use super::clip;

#[derive(serde::Deserialize)]
    struct Course {
        id: u64,
        name: String,
        #[serde(default)]
        term: Option<Term>,
    }

    #[derive(serde::Deserialize)]
    struct Term {
        #[serde(default)]
        name: Option<String>,
    }

pub(super) async fn get_courses(_: serde_json::Value)->Result<String,String>{
    let token_result=std::env::var("CANVAS_API_TOKEN");
    let token:String;
    match token_result{
        Ok(apitoken)=>token=apitoken,
        Err(_)=>token="not found".to_string()
    }
    if token=="not found".to_string(){
        return Err("canvas apitoken not found ,please check your dotenv config".to_string())
    }
    let base_url="https://oc.sjtu.edu.cn".to_string();
    let response = reqwest::Client::new()
            .get(format!("{}/api/v1/courses", base_url.trim_end_matches('/')))
            .bearer_auth(&token)
            .query(&[
                ("enrollment_state", "active"),
                ("include[]", "term"),
                ("per_page", "100"),
            ])
            .send()
            .await
            .map_err(|err| format!("请求 Canvas 失败: {err}"))?;
    let status = response.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        return Err("认证失败 (401)：令牌无效或已过期，请重新生成".to_string());
    }
    if !status.is_success() {
        return Err(format!("Canvas 返回 {status}"));
    }

    let courses: Vec<Course> = response
        .json()
        .await
        .map_err(|err| format!("解析响应失败（可能被重定向到登录页）: {err}"))?;

    if courses.is_empty() {
        return Ok("没有进行中的课程。".to_string());
    }

    let mut text = format!("共 {} 门课程：\n", courses.len());
    for course in &courses {
        text.push_str(&format!("  [{:>7}]  {}\n", course.id, course.name));
        if let Some(term) = course.term.as_ref().and_then(|t| t.name.as_deref()) {
            text.push_str(&format!("            {term}\n"));
        }
    }

    Ok(clip(&text))
    
}
