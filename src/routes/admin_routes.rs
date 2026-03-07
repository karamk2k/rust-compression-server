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
use crate::models::file_model::FileRecord;
use crate::services::auth_service::AuthService;
use crate::services::file_service::FileService;

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
    Html(render_login_page(None))
}

async fn post_login(State(state): State<AppState>, Form(form): Form<LoginForm>) -> Response {
    let auth_service = AuthService::new(state.db.clone());

    match auth_service.login(&form.username, &form.password).await {
        Ok(Some(session_id)) => {
            with_session_cookie(Redirect::to("/admin/files").into_response(), &session_id)
        }
        Ok(None) => (StatusCode::UNAUTHORIZED, Html(render_login_page(Some("Invalid username or password")))).into_response(),
        Err(error) => internal_error_page(&format!("authentication error: {}", error)),
    }
}

async fn get_files(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let user_id = match authenticated_user_id(&state, &headers).await {
        Some(user_id) => user_id,
        None => return Redirect::to("/admin/login").into_response(),
    };

    let auth_service = AuthService::new(state.db.clone());
    let file_service = FileService::new(
        state.db.clone(),
        state.compressor.clone(),
        state.media_transcoder.clone(),
        state.storage_backend.clone(),
        state.r2_storage.clone(),
        state.upload_dir.clone(),
    );

    let username = match auth_service.username_by_id(user_id).await {
        Ok(Some(username)) => username,
        Ok(None) => "admin".to_string(),
        Err(error) => return internal_error_page(&format!("failed to load user: {}", error)),
    };

    let files = match file_service.list_files().await {
        Ok(files) => files,
        Err(error) => return internal_error_page(&format!("failed to load files: {}", error)),
    };

    Html(render_files_page(&username, &files)).into_response()
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

fn render_login_page(error: Option<&str>) -> String {
    let error_html = error
        .map(|message| format!("<p style=\"color:#b91c1c;\">{}</p>", escape_html(message)))
        .unwrap_or_default();

    format!(
        r#"<!doctype html>
<html>
<head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width,initial-scale=1">
    <title>Admin Login</title>
    <style>
        body {{
            font-family: "Segoe UI", sans-serif;
            background: #f4f6fb;
            margin: 0;
            display: grid;
            place-items: center;
            min-height: 100vh;
        }}
        .card {{
            width: 100%;
            max-width: 360px;
            background: white;
            border-radius: 12px;
            padding: 24px;
            box-shadow: 0 16px 30px rgba(15, 23, 42, 0.1);
        }}
        input {{
            width: 100%;
            box-sizing: border-box;
            margin: 8px 0;
            padding: 10px;
            border: 1px solid #cbd5e1;
            border-radius: 8px;
        }}
        button {{
            width: 100%;
            padding: 10px;
            border: 0;
            border-radius: 8px;
            background: #0f766e;
            color: white;
            font-weight: 600;
            cursor: pointer;
        }}
    </style>
</head>
<body>
    <div class="card">
        <h2>Admin Login</h2>
        {error_html}
        <form method="post" action="/admin/login">
            <input name="username" placeholder="Username" required />
            <input name="password" type="password" placeholder="Password" required />
            <button type="submit">Sign In</button>
        </form>
    </div>
</body>
</html>"#
    )
}

fn render_files_page(username: &str, files: &[FileRecord]) -> String {
    let username_json = serde_json::to_string(username).unwrap_or_else(|_| "\"admin\"".to_string());
    let initial_files_json = serde_json::to_string(files).unwrap_or_else(|_| "[]".to_string());

    let template = r##"<!doctype html>
<html>
<head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width,initial-scale=1">
    <title>Drive Admin</title>
    <script src="https://unpkg.com/vue@3/dist/vue.global.prod.js"></script>
    <style>
        :root {
            --bg: #edf2ff;
            --panel: #ffffff;
            --line: #dbe5ff;
            --text: #111827;
            --muted: #5b647a;
            --primary: #2563eb;
            --danger: #b91c1c;
            --shadow: 0 16px 34px rgba(16, 24, 40, 0.08);
        }
        * {
            box-sizing: border-box;
        }
        body {
            margin: 0;
            font-family: "Sora", "Manrope", "Segoe UI", sans-serif;
            color: var(--text);
            background:
                radial-gradient(circle at 10% 20%, #dde7ff 0%, transparent 34%),
                radial-gradient(circle at 92% 6%, #d6ecff 0%, transparent 28%),
                var(--bg);
        }
        .shell {
            max-width: 1240px;
            margin: 22px auto 32px;
            padding: 0 18px;
        }
        .panel {
            background: var(--panel);
            border: 1px solid var(--line);
            border-radius: 18px;
            box-shadow: var(--shadow);
            margin-bottom: 18px;
        }
        .topbar {
            padding: 20px;
            display: flex;
            align-items: center;
            justify-content: space-between;
            gap: 12px;
            flex-wrap: wrap;
        }
        .title {
            margin: 0;
            font-size: 30px;
            font-weight: 700;
            letter-spacing: -0.5px;
        }
        .subtitle {
            margin: 4px 0 0;
            color: var(--muted);
            font-size: 14px;
        }
        .top-actions {
            display: flex;
            gap: 8px;
            flex-wrap: wrap;
        }
        .upload-panel {
            padding: 20px;
        }
        .upload-row {
            display: flex;
            align-items: center;
            gap: 10px;
            flex-wrap: wrap;
        }
        .picker {
            display: inline-flex;
            align-items: center;
            gap: 10px;
            border: 1px dashed #9ca3af;
            border-radius: 10px;
            padding: 8px 12px;
            background: #f8faff;
            min-width: 280px;
        }
        .picker input[type=file] {
            font-size: 12px;
            max-width: 220px;
        }
        .hint {
            margin: 10px 0 0;
            color: #dc2626;
            font-size: 13px;
        }
        .files-panel {
            padding: 16px;
        }
        .section-head {
            display: flex;
            justify-content: space-between;
            align-items: center;
            gap: 10px;
            padding: 6px 8px 14px;
        }
        .section-head h2 {
            margin: 0;
            font-size: 21px;
        }
        .section-stats {
            display: flex;
            align-items: center;
            justify-content: flex-end;
            gap: 8px;
            flex-wrap: wrap;
        }
        .count {
            font-size: 13px;
            color: var(--muted);
            background: #eef2ff;
            padding: 4px 10px;
            border-radius: 999px;
        }
        .summary {
            font-size: 13px;
            color: #0f172a;
            background: #dbeafe;
            padding: 4px 10px;
            border-radius: 999px;
        }
        .grid {
            display: grid;
            grid-template-columns: repeat(auto-fill, minmax(240px, 1fr));
            gap: 16px;
        }
        .file-card {
            background: #ffffff;
            border: 1px solid #dbeafe;
            border-radius: 14px;
            overflow: hidden;
            box-shadow: 0 8px 20px rgba(29, 78, 216, 0.08);
            transition: transform .18s ease, box-shadow .18s ease;
        }
        .file-card:hover {
            transform: translateY(-3px);
            box-shadow: 0 16px 30px rgba(29, 78, 216, 0.15);
        }
        .preview {
            position: relative;
            background: #eef2ff;
            min-height: 154px;
            display: grid;
            place-items: center;
        }
        .preview img,
        .preview video {
            width: 100%;
            height: 180px;
            object-fit: cover;
            display: block;
            background: #dbeafe;
        }
        .fallback {
            width: 82px;
            height: 102px;
            border-radius: 12px;
            display: grid;
            place-items: center;
            font-size: 15px;
            font-weight: 700;
            letter-spacing: 0.9px;
            border: 2px solid #64748b;
            color: #0f172a;
            background: #e2e8f0;
        }
        .fallback-doc {
            background: #dbeafe;
            border-color: #2563eb;
        }
        .fallback-audio {
            background: #fee2e2;
            border-color: #ef4444;
        }
        .fallback-archive {
            background: #fef3c7;
            border-color: #f59e0b;
        }
        .chip {
            position: absolute;
            right: 10px;
            bottom: 10px;
            font-size: 11px;
            background: rgba(15, 23, 42, 0.82);
            color: white;
            font-weight: 600;
            padding: 4px 8px;
            border-radius: 999px;
        }
        .content {
            padding: 12px;
        }
        .content h3 {
            margin: 0;
            font-size: 14px;
            white-space: nowrap;
            overflow: hidden;
            text-overflow: ellipsis;
        }
        .meta {
            margin: 6px 0 0;
            font-size: 12px;
            color: var(--muted);
        }
        .card-actions {
            margin-top: 10px;
            display: flex;
            gap: 7px;
        }
        button,
        .view {
            border: 0;
            border-radius: 8px;
            font-weight: 600;
            cursor: pointer;
            transition: opacity .16s ease;
        }
        button:hover,
        .view:hover {
            opacity: 0.88;
        }
        .primary {
            background: var(--primary);
            color: white;
            padding: 10px 14px;
            font-size: 13px;
        }
        .ghost {
            background: #e2e8f0;
            color: #0f172a;
            padding: 9px 13px;
            font-size: 13px;
        }
        .ghost-link {
            display: inline-flex;
            align-items: center;
            justify-content: center;
            text-decoration: none;
            background: #e2e8f0;
            color: #0f172a;
            padding: 9px 13px;
            font-size: 13px;
            border-radius: 8px;
            font-weight: 600;
            transition: opacity .16s ease;
        }
        .ghost-link:hover {
            opacity: 0.88;
        }
        .danger {
            background: var(--danger);
            color: white;
            padding: 9px 13px;
            font-size: 13px;
        }
        .small {
            padding: 6px 11px;
            font-size: 12px;
        }
        .view {
            display: inline-flex;
            align-items: center;
            justify-content: center;
            text-decoration: none;
            background: var(--primary);
            color: white;
            padding: 6px 11px;
            font-size: 12px;
        }
        .download {
            display: inline-flex;
            align-items: center;
            justify-content: center;
            text-decoration: none;
            background: #e2e8f0;
            color: #0f172a;
            padding: 6px 11px;
            font-size: 12px;
            border-radius: 8px;
            font-weight: 600;
        }
        .empty-state {
            border: 1px dashed #9ca3af;
            border-radius: 12px;
            background: #f8faff;
            color: #475569;
            text-align: center;
            padding: 26px 16px;
            font-size: 14px;
        }
        @media (max-width: 760px) {
            .title {
                font-size: 24px;
            }
            .preview img,
            .preview video {
                height: 164px;
            }
            .picker {
                min-width: 100%;
            }
        }
    </style>
</head>
<body>
    <div id="app" class="shell">
        <header class="topbar panel">
            <div>
                <h1 class="title">Drive Console</h1>
                <p class="subtitle">Signed in as <strong v-text="username"></strong></p>
            </div>
            <div class="top-actions">
                <a class="ghost-link" href="/admin/logs/view" target="_blank" rel="noopener">View Logs</a>
                <a class="ghost-link" href="/admin/logs/download">Download Logs</a>
                <button class="ghost" type="button" @click="clearLogs">Clear Logs</button>
                <button class="ghost" type="button" @click="refreshFiles">Refresh</button>
                <button class="danger" type="button" @click="logout">Logout</button>
            </div>
        </header>

        <section class="upload-panel panel">
            <form class="upload-row" @submit.prevent="submitUpload">
                <label class="picker">
                    <input ref="fileInput" type="file" @change="onFileChange" required>
                    <span v-text="selectedFileName || 'Choose file to upload'"></span>
                </label>
                <button class="primary" type="submit" :disabled="uploading || !selectedFile">
                    <span v-if="uploading">Uploading...</span>
                    <span v-else>Upload & Compress</span>
                </button>
            </form>
            <p class="hint" v-if="uploadError" v-text="uploadError"></p>
        </section>

        <section class="files-panel panel">
            <div class="section-head">
                <h2>Files</h2>
                <div class="section-stats">
                    <span class="count" v-text="files.length + ' items'"></span>
                    <span class="summary" v-text="'Total original: ' + formatBytes(totalOriginalBytes())"></span>
                    <span class="summary" v-text="'Total stored: ' + formatBytes(totalStoredBytes())"></span>
                </div>
            </div>

            <div class="grid" v-if="files.length > 0">
                <article class="file-card" v-for="file in files" :key="file.id">
                    <div class="preview">
                        <img v-if="isImage(file.original_name)" :src="viewUrl(file.id)" :alt="file.original_name" loading="lazy">
                        <video v-else-if="isVideo(file.original_name)" :src="viewUrl(file.id)" muted preload="metadata"></video>
                        <div v-else class="fallback" :class="'fallback-' + fileGroup(file.original_name)">
                            <span v-text="fileBadge(file.original_name)"></span>
                        </div>
                        <span class="chip" v-if="isVideo(file.original_name)">VIDEO</span>
                    </div>
                    <div class="content">
                        <h3 :title="file.original_name" v-text="file.original_name"></h3>
                        <p class="meta" v-text="'Uploaded: ' + file.created_at"></p>
                        <p class="meta" v-text="'Original: ' + formatBytes(file.original_size)"></p>
                        <p class="meta" v-text="'Compressed: ' + formatBytes(file.compressed_size)"></p>
                        <p class="meta" v-text="'Ratio: ' + compressionRatio(file) + '%'"></p>
                        <p class="meta" v-text="'Storage: ' + storageLabel(file)"></p>
                        <div class="card-actions">
                            <a class="view" :href="viewUrl(file.id)" target="_blank" rel="noopener">View</a>
                            <a class="download" :href="downloadUrl(file.id)">Download</a>
                            <button class="danger small" type="button" :disabled="deleting[file.id]" @click="deleteFile(file)">
                                <span v-if="deleting[file.id]">Deleting...</span>
                                <span v-else>Delete</span>
                            </button>
                        </div>
                    </div>
                </article>
            </div>
            <div class="empty-state" v-else>No files yet. Upload your first file.</div>
        </section>
    </div>

    <script>
        const currentUser = __USERNAME__;
        const initialFiles = __INITIAL_FILES__;

        const extension = (fileName) => {
            const index = fileName.lastIndexOf(".");
            if (index === -1) return "";
            return fileName.slice(index + 1).toLowerCase();
        };

        Vue.createApp({
            data() {
                return {
                    username: currentUser,
                    files: initialFiles,
                    selectedFile: null,
                    selectedFileName: "",
                    uploading: false,
                    uploadError: "",
                    deleting: {},
                };
            },
            methods: {
                isImage(fileName) {
                    return ["jpg", "jpeg", "png", "gif", "webp", "bmp", "svg"].includes(extension(fileName));
                },
                isVideo(fileName) {
                    return ["mp4", "webm", "mov", "avi", "mkv"].includes(extension(fileName));
                },
                fileGroup(fileName) {
                    const ext = extension(fileName);
                    if (["pdf", "doc", "docx", "txt", "md", "json", "csv", "xls", "xlsx", "ppt", "pptx"].includes(ext)) {
                        return "doc";
                    }
                    if (["mp3", "wav", "flac", "ogg"].includes(ext)) {
                        return "audio";
                    }
                    if (["zip", "rar", "7z", "tar", "gz"].includes(ext)) {
                        return "archive";
                    }
                    return "other";
                },
                fileBadge(fileName) {
                    const group = this.fileGroup(fileName);
                    if (group === "doc") return "DOC";
                    if (group === "audio") return "AUD";
                    if (group === "archive") return "ZIP";
                    return "FILE";
                },
                formatBytes(bytes) {
                    if (bytes < 1024) return bytes + " B";
                    if (bytes < 1024 * 1024) return (bytes / 1024).toFixed(1) + " KB";
                    if (bytes < 1024 * 1024 * 1024) return (bytes / (1024 * 1024)).toFixed(1) + " MB";
                    return (bytes / (1024 * 1024 * 1024)).toFixed(1) + " GB";
                },
                compressionRatio(file) {
                    if (!file.original_size) return "0.0";
                    return ((file.compressed_size / file.original_size) * 100).toFixed(1);
                },
                totalOriginalBytes() {
                    return this.files.reduce((sum, file) => sum + (Number(file.original_size) || 0), 0);
                },
                totalStoredBytes() {
                    return this.files.reduce((sum, file) => sum + (Number(file.compressed_size) || 0), 0);
                },
                storageLabel(file) {
                    if (file.is_compressed) {
                        return "zstd compressed";
                    }
                    if (file.compressed_size < file.original_size) {
                        return "media transcoded";
                    }
                    return "kept original (already optimized)";
                },
                viewUrl(fileId) {
                    return "/api/files/" + fileId + "/view";
                },
                downloadUrl(fileId) {
                    return "/api/files/" + fileId + "/download";
                },
                async refreshFiles() {
                    const response = await fetch("/api/files");
                    if (response.status === 401) {
                        window.location.href = "/admin/login";
                        return;
                    }
                    if (!response.ok) {
                        return;
                    }
                    this.files = await response.json();
                },
                onFileChange(event) {
                    const file = event.target.files && event.target.files[0] ? event.target.files[0] : null;
                    this.selectedFile = file;
                    this.selectedFileName = file ? file.name : "";
                },
                async submitUpload() {
                    if (!this.selectedFile) {
                        return;
                    }

                    this.uploading = true;
                    this.uploadError = "";

                    const body = new FormData();
                    body.append("file", this.selectedFile);

                    try {
                        const response = await fetch("/api/upload", { method: "POST", body });
                        if (!response.ok) {
                            const data = await response.json().catch(() => ({}));
                            if (response.status === 413) {
                                this.uploadError = "File is too large for current upload limit.";
                            } else {
                                this.uploadError = data.error || `Upload failed (HTTP ${response.status})`;
                            }
                            return;
                        }
                        await this.refreshFiles();
                        this.selectedFile = null;
                        this.selectedFileName = "";
                        if (this.$refs.fileInput) {
                            this.$refs.fileInput.value = "";
                        }
                    } catch (_err) {
                        this.uploadError = "Network error while uploading";
                    } finally {
                        this.uploading = false;
                    }
                },
                async deleteFile(file) {
                    if (!window.confirm("Delete " + file.original_name + "?")) {
                        return;
                    }

                    this.deleting[file.id] = true;
                    try {
                        const response = await fetch("/api/files/" + file.id, { method: "DELETE" });
                        if (!response.ok) {
                            const data = await response.json().catch(() => ({ error: "Delete failed" }));
                            this.uploadError = data.error || "Delete failed";
                            return;
                        }
                        this.files = this.files.filter((item) => item.id !== file.id);
                    } catch (_err) {
                        this.uploadError = "Network error while deleting";
                    } finally {
                        delete this.deleting[file.id];
                    }
                },
                async clearLogs() {
                    if (!window.confirm("Clear application log file?")) {
                        return;
                    }
                    try {
                        const response = await fetch("/admin/logs/clear", { method: "POST" });
                        if (response.status === 401) {
                            window.location.href = "/admin/login";
                            return;
                        }
                        if (!response.ok) {
                            const data = await response.json().catch(() => ({ error: "Failed to clear logs" }));
                            this.uploadError = data.error || "Failed to clear logs";
                            return;
                        }
                        this.uploadError = "";
                        window.alert("Logs cleared");
                    } catch (_err) {
                        this.uploadError = "Network error while clearing logs";
                    }
                },
                async logout() {
                    await fetch("/admin/logout", { method: "POST" });
                    window.location.href = "/admin/login";
                },
            },
            mounted() {
                this.refreshFiles();
            },
        }).mount("#app");
    </script>
</body>
</html>"##;

    template
        .replace("__USERNAME__", &username_json)
        .replace("__INITIAL_FILES__", &initial_files_json)
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
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width,initial-scale=1">
    <title>Logs - {}</title>
    <style>
        body {{
            margin: 0;
            font-family: "Sora", "Manrope", "Segoe UI", sans-serif;
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
    <div class="wrap">
        <div class="head">
            <h1 class="title">Log Viewer: {}</h1>
            <a class="back" href="/admin/files">Back</a>
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
