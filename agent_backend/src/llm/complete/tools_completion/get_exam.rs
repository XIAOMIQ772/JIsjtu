use super::clip;

#[derive(serde::Deserialize)]
    struct Assignment {
        id: u64,
        name: String,
        #[serde(default)]
        due_at: Option<String>,
        #[serde(default)]
        points_possible: Option<f64>,
        #[serde(default)]
        html_url: Option<String>,
    }

#[derive(serde::Deserialize)]
struct AssignmentDetail {
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    submission_types: Vec<String>,
    #[serde(default)]
    allowed_attempts: Option<i64>,
    #[serde(default)]
    unlock_at: Option<String>,
    #[serde(default)]
    lock_at: Option<String>,
}

pub(super) async fn get_exam(arguments: serde_json::Value)->Result<String,String>{
    let course_id=arguments.get("course_id").unwrap().as_str();
    let mut base_url="https://oc.sjtu.edu.cn/courses/".to_string();
    match course_id{
        Some(id)=>base_url.push_str(id),
        None=>()
    }
    if base_url=="https://oc.sjtu.edu.cn/courses/".to_string(){
        return Err("course not found".to_string())
    }
    let id = course_id.ok_or_else(|| "course not found".to_string())?;

    let token = std::env::var("CANVAS_API_TOKEN")
        .map_err(|_| "canvas apitoken not found ,please check your dotenv config".to_string())?;

    let api_url = format!("https://oc.sjtu.edu.cn/api/v1/courses/{id}/assignments");

    let client = reqwest::Client::new();

    let response = client
        .get(&api_url)
        .bearer_auth(&token)
        .query(&[("per_page", "100")])
        .send()
        .await
        .map_err(|err| format!("请求 Canvas 失败: {err}"))?;

    let status = response.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        return Err("认证失败 (401)：令牌无效或已过期".to_string());
    }
    if status == reqwest::StatusCode::NOT_FOUND {
        return Err(format!("课程 {id} 不存在，或该课程未启用「作业」页面"));
    }
    if !status.is_success() {
        return Err(format!("Canvas 返回 {status}"));
    }

    let assignments: Vec<Assignment> = response
        .json()
        .await
        .map_err(|err| format!("解析响应失败: {err}"))?;

    if assignments.is_empty() {
        return Ok(format!("课程 {id} 还没有公布作业。"));
    }

    let mut text = format!("课程 {id} 的作业（共 {} 项）：\n", assignments.len());
    for assignment in &assignments {
        text.push_str(&format!("\n[{}] {}\n", assignment.id, assignment.name));
        text.push_str(&format!(
            "截止：{}\n",
            assignment.due_at.as_deref().unwrap_or("未设置")
        ));
        if let Some(points) = assignment.points_possible {
            text.push_str(&format!("分值：{points}\n"));
        }
        if let Some(url) = &assignment.html_url {
            text.push_str(&format!("链接：https://oc.sjtu.edu.cn{url}\n"));
        }

        let detail_url = format!(
            "https://oc.sjtu.edu.cn/api/v1/courses/{id}/assignments/{}",
            assignment.id
        );
        let detail = client
            .get(&detail_url)
            .bearer_auth(&token)
            .send()
            .await
            .and_then(|response| response.error_for_status());

        match detail {
            Ok(response) => match response.json::<AssignmentDetail>().await {
                Ok(detail) => {
                    if let Some(unlock_at) = &detail.unlock_at {
                        text.push_str(&format!("开始：{unlock_at}\n"));
                    }
                    if let Some(lock_at) = &detail.lock_at {
                        text.push_str(&format!("锁定：{lock_at}\n"));
                    }
                    if let Some(attempts) = detail.allowed_attempts {
                        let times = if attempts < 0 {
                            "不限".to_string()
                        } else {
                            attempts.to_string()
                        };
                        text.push_str(&format!("允许尝试：{times}\n"));
                    }
                    if !detail.submission_types.is_empty() {
                        text.push_str(&format!(
                            "提交方式：{}\n",
                            detail.submission_types.join("、")
                        ));
                    }
                    if let Some(description) = detail.description.as_deref() {
                        let description = description.trim();
                        if !description.is_empty() {
                            text.push_str(&format!("说明：{description}\n"));
                        }
                    }
                }
                Err(err) => text.push_str(&format!("详情解析失败：{err}\n")),
            },
            Err(err) => text.push_str(&format!("详情获取失败：{err}\n")),
        }
    }

    Ok(clip(&text))
}
