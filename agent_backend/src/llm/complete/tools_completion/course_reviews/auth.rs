use std::time::Duration;

use reqwest::header::{COOKIE, HeaderMap, HeaderValue, SET_COOKIE};
use reqwest::{Client, Response, StatusCode};
use serde::de::DeserializeOwned;
use serde_json::json;
use tokio::sync::{Mutex, Semaphore};

const API: &str = "https://course.sjtu.plus/api";
const CSRF_HEADER: &str = "x-csrf-token";

struct Session {
    email: String,
    cookie: HeaderValue,
}

static SESSION: Mutex<Option<Session>> = Mutex::const_new(None);
static REQUESTS: Semaphore = Semaphore::const_new(4);

pub(super) struct CourseReviewClient {
    http: Client,
    email: String,
    password: String,
}

impl CourseReviewClient {
    pub(super) fn new() -> Result<Self, String> {
        let email = std::env::var("JCOURSE_EMAIL").unwrap_or_default();
        let password = std::env::var("JCOURSE_PASSWORD").unwrap_or_default();
        if email.trim().is_empty() || password.is_empty() {
            return Err("请在 .env 中设置 JCOURSE_EMAIL 和 JCOURSE_PASSWORD，分别填写选课社区账号和社区密码。".into());
        }
        let email = email.trim().to_ascii_lowercase();
        let username = email.strip_suffix("@sjtu.edu.cn").unwrap_or(&email);
        if username.is_empty()
            || !username
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-+".contains(&b))
        {
            return Err("JCOURSE_EMAIL 请填写账号前缀或完整的 @sjtu.edu.cn 邮箱。".into());
        }
        let http = Client::builder()
            .https_only(true)
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(15))
            .build()
            .map_err(|_| "无法初始化选课社区连接。")?;
        Ok(Self {
            http,
            email: format!("{username}@sjtu.edu.cn"),
            password,
        })
    }

    pub(super) async fn get<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<T, String> {
        if !(path.starts_with("/course/") || path.starts_with("/teacher/"))
            || !path.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'/')
        {
            return Err("选课社区查询路径无效。".into());
        }
        let _permit = REQUESTS
            .acquire()
            .await
            .map_err(|_| "选课社区连接已关闭。")?;
        let request = |cookie: HeaderValue| {
            self.http
                .get(format!("{API}{path}"))
                .query(query)
                .header(COOKIE, cookie)
                .send()
        };
        let (cookie, reused) = self.session(None).await?;
        let mut response = request(cookie.clone()).await.map_err(connection_error)?;
        if reused && response.status() == StatusCode::UNAUTHORIZED {
            // Another concurrent query may have refreshed this same cookie already.
            let (fresh, _) = self.session(Some(&cookie)).await?;
            response = request(fresh).await.map_err(connection_error)?;
        }
        checked(response, "查询")?
            .json()
            .await
            .map_err(|_| "无法解析选课社区返回的数据。".into())
    }

    async fn session(&self, rejected: Option<&HeaderValue>) -> Result<(HeaderValue, bool), String> {
        let mut session = SESSION.lock().await;
        if let Some(cached) = &*session
            && cached.email == self.email
            && Some(&cached.cookie) != rejected
        {
            return Ok((cached.cookie.clone(), true));
        }
        *session = None;
        let cookie = self.login().await?;
        *session = Some(Session {
            email: self.email.clone(),
            cookie: cookie.clone(),
        });
        Ok((cookie, false))
    }

    async fn login(&self) -> Result<HeaderValue, String> {
        let csrf = self
            .http
            .get(format!("{API}/auth/csrf"))
            .send()
            .await
            .map_err(connection_error)?;
        let csrf = checked(csrf, "获取登录令牌")?;
        let cookie = session_cookie(csrf.headers())?;
        let mut token = csrf
            .headers()
            .get(CSRF_HEADER)
            .filter(|v| !v.is_empty())
            .cloned()
            .ok_or("选课社区未返回登录令牌。")?;
        token.set_sensitive(true);
        let login = self
            .http
            .post(format!("{API}/auth/login"))
            .header(COOKIE, cookie)
            .header(CSRF_HEADER, token)
            .json(&json!({"email": self.email, "password": self.password}))
            .send()
            .await
            .map_err(connection_error)?;
        if login.status() == StatusCode::UNAUTHORIZED {
            return Err(
                "选课社区未接受邮箱或社区密码，请检查 JCOURSE_EMAIL 和 JCOURSE_PASSWORD。".into(),
            );
        }
        session_cookie(checked(login, "登录")?.headers())
    }
}

fn session_cookie(headers: &HeaderMap) -> Result<HeaderValue, String> {
    // Login expires the anonymous cookie and issues a new session with the same name.
    let mut cookie = headers
        .get_all(SET_COOKIE)
        .iter()
        .rev()
        .find_map(|header| {
            let pair = header.to_str().ok()?.split(';').next()?.trim();
            let value = pair.strip_prefix("jcourse_session=")?;
            if value.is_empty() {
                return None;
            }
            HeaderValue::from_str(pair).ok()
        })
        .ok_or("选课社区未返回登录会话。")?;
    cookie.set_sensitive(true);
    Ok(cookie)
}

fn checked(response: Response, action: &str) -> Result<Response, String> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let reason = match status {
        StatusCode::UNAUTHORIZED => "登录会话未被接受",
        StatusCode::FORBIDDEN => "社区拒绝访问",
        StatusCode::TOO_MANY_REQUESTS => "请求太频繁，请稍后再试",
        _ => "社区接口返回错误",
    };
    Err(format!(
        "选课社区{action}失败（HTTP {}），{reason}。",
        status.as_u16()
    ))
}

fn connection_error(err: reqwest::Error) -> String {
    if err.is_timeout() {
        "连接选课社区超时。"
    } else {
        "无法连接选课社区。"
    }
    .into()
}
