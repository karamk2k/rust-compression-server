use axum::body::Body;
use axum::extract::{Form, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncSeekExt, SeekFrom};
use tokio_util::io::ReaderStream;

use crate::app_state::AppState;
use crate::auth::{
    authenticated_user_id, clear_session_cookie, session_id_from_headers, with_session_cookie,
};
use crate::services::auth_service::AuthService;

const LOGIN_PAGE_HTML: &str = include_str!("../../web/admin/login.html");
const FILES_PAGE_HTML: &str = include_str!("../../web/admin/files.html");

#[derive(Debug, Deserialize)]
struct LoginForm {
    username: String,
    password: String,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/admin/login", get(get_login).post(post_login))
        .route("/admin/files", get(get_files))
        .route("/admin/logs/view", get(view_logs))
        .route("/admin/logs/download", get(download_logs))
        .route("/admin/logs/clear", post(clear_logs))
        .route("/admin/logout", post(post_logout))
}

async fn get_login() -> Html<String> {
    Html(LOGIN_PAGE_HTML.to_string())
}

async fn post_login(State(state): State<AppState>, Form(form): Form<LoginForm>) -> Response {
    let auth_service = AuthService::new(state.db.clone());

    match auth_service.login(&form.username, &form.password).await {
        Ok(Some(session_id)) => {
            with_session_cookie(Redirect::to("/admin/files").into_response(), &session_id)
        }
        Ok(None) => Redirect::to("/admin/login?error=invalid_credentials").into_response(),
        Err(error) => internal_error_page(&format!("authentication error: {}", error)),
    }
}

async fn get_files(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if authenticated_user_id(&state, &headers).await.is_none() {
        return Redirect::to("/admin/login").into_response();
    }

    Html(FILES_PAGE_HTML.to_string()).into_response()
}

async fn post_logout(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Some(session_id) = session_id_from_headers(&headers) {
        let auth_service = AuthService::new(state.db.clone());
        let _ = auth_service.logout(&session_id).await;
    }

    clear_session_cookie(Redirect::to("/admin/login").into_response())
}

async fn view_logs(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if authenticated_user_id(&state, &headers).await.is_none() {
        return Redirect::to("/admin/login").into_response();
    }

    let file_name = state
        .log_file
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("app.log")
        .to_string();

    let body = match read_log_tail(&state.log_file, 1024 * 1024).await {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            "Log file does not exist yet.".to_string()
        }
        Err(error) => {
            return internal_error_page(&format!("failed to read logs: {}", error));
        }
    };

    Html(render_log_page(&file_name, &body)).into_response()
}

async fn download_logs(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if authenticated_user_id(&state, &headers).await.is_none() {
        return Redirect::to("/admin/login").into_response();
    }

    let file = match File::open(&state.log_file).await {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let mut response = Response::new(Body::empty());
            *response.status_mut() = StatusCode::OK;
            response
                .headers_mut()
                .insert(header::CONTENT_TYPE, HeaderValue::from_static("text/plain; charset=utf-8"));
            response
                .headers_mut()
                .insert(header::CONTENT_LENGTH, HeaderValue::from_static("0"));
            response
                .headers_mut()
                .insert(
                    header::CONTENT_DISPOSITION,
                    HeaderValue::from_static("attachment; filename=\"app.log\""),
                );
            return response;
        }
        Err(error) => {
            return internal_error_page(&format!("failed to open logs: {}", error));
        }
    };

    let metadata = match file.metadata().await {
        Ok(metadata) => metadata,
        Err(error) => return internal_error_page(&format!("failed to stat logs: {}", error)),
    };
    let file_name = state
        .log_file
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("app.log");

    let stream = ReaderStream::new(file);
    let mut response = Response::new(Body::from_stream(stream));
    *response.status_mut() = StatusCode::OK;
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static("text/plain; charset=utf-8"));
    if let Ok(value) = HeaderValue::from_str(&metadata.len().to_string()) {
        response
            .headers_mut()
            .insert(header::CONTENT_LENGTH, value);
    }
    if let Ok(value) = HeaderValue::from_str(&format!("attachment; filename=\"{}\"", file_name.replace('"', "_"))) {
        response
            .headers_mut()
            .insert(header::CONTENT_DISPOSITION, value);
    }
    response
}

async fn clear_logs(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if authenticated_user_id(&state, &headers).await.is_none() {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "unauthorized" })),
        )
            .into_response();
    }

    if let Err(error) = tokio::fs::write(&state.log_file, "").await {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("failed to clear logs: {}", error) })),
        )
            .into_response();
    }

    Json(json!({ "cleared": true })).into_response()
}

fn internal_error_page(message: &str) -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Html(format!(
            "<h1>Internal Server Error</h1><pre>{}</pre>",
            escape_html(message)
        )),
    )
        .into_response()
}

async fn read_log_tail(path: &std::path::Path, max_bytes: u64) -> std::io::Result<String> {
    let mut file = File::open(path).await?;
    let metadata = file.metadata().await?;
    let file_size = metadata.len();
    let start_offset = file_size.saturating_sub(max_bytes);

    file.seek(SeekFrom::Start(start_offset)).await?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf).await?;

    let mut text = String::from_utf8_lossy(&buf).to_string();
    if start_offset > 0 {
        text = format!(
            "... showing last {} bytes of {} bytes ...\n\n{}",
            file_size - start_offset,
            file_size,
            text
        );
    }
    Ok(text)
}

fn render_log_page(file_name: &str, body: &str) -> String {
    format!(
        r#"<!doctype html>
<html>
<head>
    <meta charset=\"utf-8\">
    <meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">
    <title>Logs - {}</title>
    <style>
        body {{
            margin: 0;
            font-family: \"Sora\", \"Manrope\", \"Segoe UI\", sans-serif;
            background: #0b1220;
            color: #dbeafe;
        }}
        .wrap {{
            max-width: 1200px;
            margin: 20px auto;
            padding: 0 16px;
        }}
        .head {{
            display: flex;
            align-items: center;
            justify-content: space-between;
            gap: 10px;
            margin-bottom: 12px;
        }}
        .title {{
            margin: 0;
            font-size: 18px;
            color: #93c5fd;
        }}
        .back {{
            display: inline-flex;
            text-decoration: none;
            color: #111827;
            background: #bfdbfe;
            border-radius: 8px;
            padding: 7px 11px;
            font-weight: 600;
            font-size: 13px;
        }}
        pre {{
            margin: 0;
            white-space: pre-wrap;
            word-break: break-word;
            background: #020617;
            border: 1px solid #1e293b;
            border-radius: 12px;
            padding: 14px;
            line-height: 1.35;
            font-size: 12px;
        }}
    </style>
</head>
<body>
    <div class=\"wrap\">
        <div class=\"head\">
            <h1 class=\"title\">Log Viewer: {}</h1>
            <a class=\"back\" href=\"/admin/files\">Back</a>
        </div>
        <pre>{}</pre>
    </div>
</body>
</html>"#,
        escape_html(file_name),
        escape_html(file_name),
        escape_html(body)
    )
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
