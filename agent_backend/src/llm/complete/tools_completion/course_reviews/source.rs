use serde::{Deserialize, de::DeserializeOwned};

use super::auth::CourseReviewClient;

const SITE: &str = "https://course.sjtu.plus";

#[derive(Debug, Deserialize)]
pub(super) struct Page<T> {
    pub(super) items: Vec<T>,
    pub(super) total: usize,
    pub(super) page: usize,
    pub(super) page_size: usize,
}

#[derive(Clone, Debug, Deserialize)]
pub(super) struct Teacher {
    pub(super) id: u64,
    pub(super) name: String,
    pub(super) department: String,
    #[serde(default)]
    pub(super) title: String,
}

impl Teacher {
    pub(super) fn url(&self) -> String {
        format!("{SITE}/teacher/{}", self.id)
    }
}

#[derive(Clone, Debug, Deserialize)]
pub(super) struct Rating {
    pub(super) count: usize,
    pub(super) avg: f64,
    // Index 0 is one star. Only the detail endpoint populates these counts.
    pub(super) distribution: [usize; 5],
}

#[derive(Clone, Debug, Deserialize)]
pub(super) struct Course {
    pub(super) id: u64,
    pub(super) code: String,
    pub(super) name: String,
    pub(super) department: String,
    #[serde(default)]
    pub(super) last_semester: String,
    pub(super) main_teacher: Teacher,
    // jCourse includes the full teaching team only on the course detail page.
    #[serde(default)]
    pub(super) teacher_group: Vec<Teacher>,
    pub(super) rating: Rating,
}

impl Course {
    pub(super) fn url(&self) -> String {
        format!("{SITE}/course/{}", self.id)
    }
}

#[derive(Clone, Debug, Deserialize)]
pub(super) struct Review {
    pub(super) id: u64,
    pub(super) course_id: u64,
    #[serde(default)]
    pub(super) semester: String,
    pub(super) rating: u8,
    pub(super) content: String,
    pub(super) created_at: String,
}

impl Review {
    pub(super) fn url(&self) -> String {
        format!("{SITE}/review/{}", self.id)
    }
}

#[derive(Clone, Copy)]
pub(super) enum ReviewOrder {
    Recent,
    Popular,
}

async fn page<T: DeserializeOwned>(
    client: &CourseReviewClient,
    path: &str,
    query: &str,
    mut params: Vec<(&str, String)>,
    page_number: usize,
    page_size: usize,
) -> Result<Page<T>, String> {
    if page_number == 0 || !(1..=100).contains(&page_size) {
        return Err("选课社区的页码从 1 开始，每页支持 1 到 100 条记录。".into());
    }
    if !query.trim().is_empty() {
        params.push(("q", query.trim().to_owned()));
    }
    params.push(("page", page_number.to_string()));
    params.push(("page_size", page_size.to_string()));
    let result: Page<T> = client.get(path, &params).await?;
    if result.page != page_number || result.page_size != page_size || result.items.len() > page_size
    {
        return Err("选课社区返回的分页与请求不一致，无法确定查询范围。".into());
    }
    Ok(result)
}

pub(super) async fn teachers(
    client: &CourseReviewClient,
    query: &str,
    page_number: usize,
    page_size: usize,
) -> Result<Page<Teacher>, String> {
    if query.trim().is_empty() {
        return Err("请提供要查询的教师姓名。".into());
    }
    page(client, "/teacher/", query, vec![], page_number, page_size).await
}

pub(super) async fn teacher(client: &CourseReviewClient, id: u64) -> Result<Teacher, String> {
    if id == 0 {
        return Err("选课社区的教师 ID 必须大于 0。".into());
    }
    let teacher: Teacher = client.get(&format!("/teacher/{id}"), &[]).await?;
    if teacher.id != id {
        return Err("选课社区返回的教师与请求的教师 ID 不一致。".into());
    }
    Ok(teacher)
}

pub(super) async fn courses(
    client: &CourseReviewClient,
    query: &str,
    teacher_id: Option<u64>,
    page_number: usize,
    page_size: usize,
) -> Result<Page<Course>, String> {
    let path = match teacher_id {
        Some(0) => return Err("选课社区的教师 ID 必须大于 0。".into()),
        Some(id) => format!("/teacher/{id}/course"),
        None if query.trim().is_empty() => return Err("请提供课程名称、课程代码或教师。".into()),
        None => "/course/".to_owned(),
    };
    page(client, &path, query, vec![], page_number, page_size).await
}

pub(super) async fn course(client: &CourseReviewClient, id: u64) -> Result<Course, String> {
    if id == 0 {
        return Err("选课社区的课程 ID 必须大于 0。".into());
    }
    let course: Course = client.get(&format!("/course/{id}"), &[]).await?;
    if course.id != id {
        return Err("选课社区返回的授课记录与请求的课程 ID 不一致。".into());
    }
    Ok(course)
}

pub(super) async fn reviews(
    client: &CourseReviewClient,
    course_id: u64,
    keyword: Option<&str>,
    rating: Option<u8>,
    order: ReviewOrder,
    page_number: usize,
    page_size: usize,
) -> Result<Page<Review>, String> {
    if course_id == 0 {
        return Err("选课社区的课程 ID 必须大于 0。".into());
    }
    let order_by = match order {
        ReviewOrder::Recent => "created_at",
        ReviewOrder::Popular => "like_count",
    };
    let path = format!("/course/{course_id}/review");
    let mut params = vec![("order_by", order_by.to_owned())];
    if let Some(stars) = rating {
        if !(1..=5).contains(&stars) {
            return Err("选课社区的评分范围是 1 到 5 星。".into());
        }
        params.push(("rating", stars.to_string()));
    }
    let result: Page<Review> = page(
        client,
        &path,
        keyword.unwrap_or_default(),
        params,
        page_number,
        page_size,
    )
    .await?;
    if result.items.iter().any(|r| r.course_id != course_id) {
        return Err("选课社区返回了其他授课记录的点评，无法归入本课程。".into());
    }
    Ok(result)
}
