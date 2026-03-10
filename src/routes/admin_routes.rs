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
use crate::db;
use crate::models::file_model::FileRecord;
use crate::models::folder_model::FolderListItem;
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

    let files = match file_service.list_files_for_user(user_id, None).await {
        Ok(files) => files,
        Err(error) => return internal_error_page(&format!("failed to load files: {}", error)),
    };
    let folders = match db::list_folders_for_user_and_parent(&state.db, user_id, None).await {
        Ok(folders) => folders,
        Err(error) => return internal_error_page(&format!("failed to load folders: {}", error)),
    };

    Html(render_files_page(&username, &files, &folders)).into_response()
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

fn render_files_page(username: &str, files: &[FileRecord], folders: &[FolderListItem]) -> String {
    let username_json = serde_json::to_string(username).unwrap_or_else(|_| "\"admin\"".to_string());
    let initial_files_json = serde_json::to_string(files).unwrap_or_else(|_| "[]".to_string());
    let initial_folders_json = serde_json::to_string(folders).unwrap_or_else(|_| "[]".to_string());

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
        * { box-sizing: border-box; }
        body {
            margin: 0;
            font-family: "Sora", "Manrope", "Segoe UI", sans-serif;
            color: var(--text);
            background:
                radial-gradient(circle at 10% 20%, #dde7ff 0%, transparent 34%),
                radial-gradient(circle at 92% 6%, #d6ecff 0%, transparent 28%),
                var(--bg);
        }
        .shell { max-width: 1240px; margin: 22px auto 32px; padding: 0 18px; }
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
        .title { margin: 0; font-size: 30px; font-weight: 700; letter-spacing: -0.5px; }
        .subtitle { margin: 4px 0 0; color: var(--muted); font-size: 14px; }
        .top-actions { display: flex; gap: 8px; flex-wrap: wrap; }
        .upload-panel { padding: 20px; }
        .upload-controls {
            display: flex;
            gap: 10px;
            align-items: center;
            flex-wrap: wrap;
            margin-bottom: 12px;
        }
        .drop-zone {
            border: 1px dashed #9ca3af;
            border-radius: 12px;
            background: #f8faff;
            min-height: 96px;
            display: grid;
            place-items: center;
            text-align: center;
            color: var(--muted);
            padding: 14px;
            transition: border-color .2s ease, background .2s ease;
        }
        .drop-zone.active {
            border-color: #2563eb;
            background: #eaf2ff;
            color: #1d4ed8;
        }
        .picker { display: inline-flex; align-items: center; gap: 8px; }
        .picker input[type=file] { max-width: 300px; }
        .hint { margin: 10px 0 0; color: #dc2626; font-size: 13px; }
        .hint.ok { color: #2563eb; }
        .nav-panel { padding: 16px; }
        .nav-row {
            display: flex;
            justify-content: space-between;
            align-items: center;
            gap: 10px;
            flex-wrap: wrap;
        }
        .breadcrumbs { display: flex; gap: 6px; align-items: center; flex-wrap: wrap; }
        .crumb {
            background: #eef2ff;
            border: 0;
            border-radius: 999px;
            color: #1f2937;
            font-size: 12px;
            padding: 5px 10px;
            cursor: pointer;
        }
        .crumb.current { background: #dbeafe; font-weight: 700; color: #1d4ed8; }
        .folder-grid {
            margin-top: 14px;
            display: grid;
            grid-template-columns: repeat(auto-fill, minmax(220px, 1fr));
            gap: 12px;
        }
        .folder-card {
            border: 1px solid #c7d2fe;
            border-radius: 12px;
            background: #f8fbff;
            padding: 12px;
            transition: transform .15s ease, box-shadow .15s ease;
        }
        .folder-card:hover {
            transform: translateY(-2px);
            box-shadow: 0 10px 18px rgba(37, 99, 235, 0.14);
        }
        .folder-card.drop-target {
            border-color: #2563eb;
            background: #e8f0ff;
            box-shadow: 0 0 0 2px rgba(37, 99, 235, 0.2);
        }
        .folder-head {
            display: flex;
            align-items: center;
            justify-content: space-between;
            gap: 8px;
        }
        .folder-open {
            border: 0;
            padding: 0;
            margin: 0;
            background: transparent;
            color: #1f2937;
            font-size: 14px;
            font-weight: 700;
            cursor: pointer;
            text-align: left;
        }
        .folder-open:hover { color: #1d4ed8; }
        .folder-title { margin: 0; font-size: 14px; color: #1f2937; }
        .folder-meta { margin: 6px 0 0; font-size: 12px; color: var(--muted); }
        .folder-actions {
            margin-top: 10px;
            display: flex;
            gap: 7px;
            flex-wrap: wrap;
        }
        .crumb.drop-target {
            outline: 2px solid #2563eb;
            outline-offset: 1px;
            background: #dbeafe;
        }
        .files-panel { padding: 16px; }
        .section-head {
            display: flex;
            justify-content: space-between;
            align-items: center;
            gap: 10px;
            padding: 6px 8px 14px;
        }
        .section-head h2 { margin: 0; font-size: 21px; }
        .section-stats { display: flex; align-items: center; justify-content: flex-end; gap: 8px; flex-wrap: wrap; }
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
        .file-card.dragging { opacity: 0.55; }
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
        .fallback-doc { background: #dbeafe; border-color: #2563eb; }
        .fallback-audio { background: #fee2e2; border-color: #ef4444; }
        .fallback-archive { background: #fef3c7; border-color: #f59e0b; }
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
        .content { padding: 12px; }
        .content h3 {
            margin: 0;
            font-size: 14px;
            white-space: nowrap;
            overflow: hidden;
            text-overflow: ellipsis;
        }
        .meta { margin: 6px 0 0; font-size: 12px; color: var(--muted); }
        .card-actions { margin-top: 10px; display: flex; gap: 7px; }
        button, .view {
            border: 0;
            border-radius: 8px;
            font-weight: 600;
            cursor: pointer;
            transition: opacity .16s ease;
        }
        button:hover, .view:hover { opacity: 0.88; }
        .primary { background: var(--primary); color: white; padding: 10px 14px; font-size: 13px; }
        .ghost { background: #e2e8f0; color: #0f172a; padding: 9px 13px; font-size: 13px; }
        .ghost:disabled { opacity: 0.55; cursor: not-allowed; }
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
        }
        .danger { background: var(--danger); color: white; padding: 9px 13px; font-size: 13px; }
        .small { padding: 6px 11px; font-size: 12px; }
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
            .title { font-size: 24px; }
            .preview img, .preview video { height: 164px; }
            .upload-controls { align-items: stretch; }
            .picker input[type=file] { max-width: 100%; }
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
                <button class="ghost" type="button" @click="refreshCurrentFolder">Refresh</button>
                <button class="danger" type="button" @click="logout">Logout</button>
            </div>
        </header>

        <section class="upload-panel panel">
            <div class="upload-controls">
                <label class="picker">
                    <input ref="fileInput" type="file" multiple @change="onFileChange">
                </label>
                <button class="primary" type="button" :disabled="uploading || selectedFiles.length === 0" @click="submitUpload">
                    <span v-if="uploading">Uploading...</span>
                    <span v-else>Upload {{ selectedFiles.length > 1 ? selectedFiles.length + ' Files' : 'Files' }}</span>
                </button>
                <button class="ghost" type="button" :disabled="uploading || selectedFiles.length === 0" @click="clearSelectedFiles">Clear</button>
            </div>
            <div class="drop-zone" :class="{ active: dragActive }" @dragover.prevent="onDragOver" @dragleave="onDragLeave" @drop.prevent="onDrop">
                <span v-if="selectedFiles.length === 0">Drag and drop files here, or use picker (multi-select works on mobile picker too).</span>
                <span v-else>{{ selectedFiles.length }} file(s) selected</span>
            </div>
            <p class="hint" v-if="uploadError" v-text="uploadError"></p>
            <p class="hint ok" v-if="uploadStatus" v-text="uploadStatus"></p>
        </section>

        <section class="nav-panel panel">
            <div class="nav-row">
                <div class="breadcrumbs">
                    <button
                        class="crumb"
                        :class="{ current: currentFolderId === null, 'drop-target': folderDropTargetId === 'root' }"
                        @click="openRoot"
                        @dragover.prevent="onRootDragOver"
                        @dragleave="onRootDragLeave"
                        @drop.prevent="onRootDrop"
                    >Root</button>
                    <template v-for="(node, index) in folderPath" :key="node.id">
                        <span>/</span>
                        <button class="crumb" :class="{ current: index === folderPath.length - 1 }" @click="openPath(index)">{{ node.name }}</button>
                    </template>
                </div>
                <div class="top-actions">
                    <button class="ghost" type="button" :disabled="currentFolderId === null" @click="goUp">Up</button>
                    <button class="primary" type="button" @click="createFolder">New Folder</button>
                </div>
            </div>
            <div class="folder-grid" v-if="folders.length > 0">
                <article
                    class="folder-card"
                    :class="{ 'drop-target': folderDropTargetId === folder.id }"
                    v-for="folder in folders"
                    :key="folder.id"
                    @dragover.prevent="onFolderDragOver(folder.id, $event)"
                    @dragleave="onFolderDragLeave(folder.id, $event)"
                    @drop.prevent="onFolderDrop(folder, $event)"
                >
                    <div class="folder-head">
                        <button class="folder-open" type="button" @click="openFolder(folder)">[Folder] {{ folder.name }}</button>
                    </div>
                    <p class="folder-meta">{{ folder.file_count }} item(s)</p>
                    <p class="folder-meta">Created: {{ folder.created_at }}</p>
                    <p class="folder-meta">ID: {{ folder.id }}</p>
                    <div class="folder-actions">
                        <button class="ghost small" type="button" @click="renameFolder(folder)">Rename</button>
                        <button class="ghost small" type="button" @click="moveFolder(folder)">Move</button>
                        <button class="danger small" type="button" @click="deleteFolder(folder)">Delete</button>
                    </div>
                </article>
            </div>
            <div class="empty-state" v-else>No subfolders here.</div>
        </section>

        <section class="files-panel panel">
            <div class="section-head">
                <h2>Files</h2>
                <div class="section-stats">
                    <span class="count" v-text="displayItemCountText()"></span>
                    <span class="summary" v-text="'Total original: ' + formatBytes(displayOriginalBytes())"></span>
                    <span class="summary" v-text="'Total stored: ' + formatBytes(displayStoredBytes())"></span>
                </div>
            </div>
            <div class="grid" v-if="files.length > 0">
                <article
                    class="file-card"
                    :class="{ dragging: draggingFileId === file.id }"
                    v-for="file in files"
                    :key="file.id"
                    draggable="true"
                    @dragstart="onFileDragStart(file, $event)"
                    @dragend="onFileDragEnd"
                >
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
            <div class="empty-state" v-else>No files in this folder yet.</div>
        </section>
    </div>

    <script>
        const currentUser = __USERNAME__;
        const initialFiles = __INITIAL_FILES__;
        const initialFolders = __INITIAL_FOLDERS__;

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
                    folders: initialFolders,
                    folderPath: [],
                    currentFolderId: null,
                    selectedFiles: [],
                    uploading: false,
                    uploadError: "",
                    uploadStatus: "",
                    deleting: {},
                    dragActive: false,
                    draggingFileId: null,
                    folderDropTargetId: null,
                    fileSummary: {
                        total_files: initialFiles.length,
                        total_original_size: initialFiles.reduce((sum, file) => sum + (Number(file.original_size) || 0), 0),
                        total_stored_size: initialFiles.reduce((sum, file) => sum + (Number(file.compressed_size) || 0), 0),
                    },
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
                displayItemCountText() {
                    if (this.currentFolderId === null) {
                        return this.fileSummary.total_files + " items (all folders)";
                    }
                    return this.files.length + " items";
                },
                displayOriginalBytes() {
                    if (this.currentFolderId === null) {
                        return Number(this.fileSummary.total_original_size) || 0;
                    }
                    return this.totalOriginalBytes();
                },
                displayStoredBytes() {
                    if (this.currentFolderId === null) {
                        return Number(this.fileSummary.total_stored_size) || 0;
                    }
                    return this.totalStoredBytes();
                },
                storageLabel(file) {
                    if (file.is_compressed) return "zstd compressed";
                    if (file.compressed_size < file.original_size) return "media transcoded";
                    return "kept original (already optimized)";
                },
                viewUrl(fileId) { return "/api/files/" + fileId + "/view"; },
                downloadUrl(fileId) { return "/api/files/" + fileId + "/download"; },
                currentParentId() {
                    if (this.currentFolderId === null) return null;
                    if (this.folderPath.length < 2) return null;
                    return this.folderPath[this.folderPath.length - 2].id;
                },
                async refreshFiles() {
                    const query = this.currentFolderId === null ? "" : ("?folder_id=" + this.currentFolderId);
                    const response = await fetch("/api/files" + query);
                    if (response.status === 401) {
                        window.location.href = "/admin/login";
                        return;
                    }
                    if (!response.ok) {
                        const data = await response.json().catch(() => ({}));
                        throw new Error(data.error || "Failed to refresh files");
                    }
                    this.files = await response.json();
                },
                async refreshFolders() {
                    const query = this.currentFolderId === null ? "" : ("?parent_id=" + this.currentFolderId);
                    const response = await fetch("/api/folders" + query);
                    if (response.status === 401) {
                        window.location.href = "/admin/login";
                        return;
                    }
                    if (!response.ok) {
                        const data = await response.json().catch(() => ({}));
                        throw new Error(data.error || "Failed to refresh folders");
                    }
                    this.folders = await response.json();
                },
                async refreshFolderPath() {
                    if (this.currentFolderId === null) {
                        this.folderPath = [];
                        return;
                    }
                    const response = await fetch("/api/folders/path?folder_id=" + this.currentFolderId);
                    if (!response.ok) {
                        const data = await response.json().catch(() => ({}));
                        throw new Error(data.error || "Failed to refresh folder path");
                    }
                    this.folderPath = await response.json();
                },
                async refreshFileSummary() {
                    const response = await fetch("/api/files/summary");
                    if (response.status === 401) {
                        window.location.href = "/admin/login";
                        return;
                    }
                    if (!response.ok) {
                        const data = await response.json().catch(() => ({}));
                        throw new Error(data.error || "Failed to refresh total summary");
                    }
                    this.fileSummary = await response.json();
                },
                async refreshCurrentFolder() {
                    this.uploadError = "";
                    await Promise.all([
                        this.refreshFolders(),
                        this.refreshFiles(),
                        this.refreshFolderPath(),
                        this.refreshFileSummary(),
                    ]);
                },
                openFolder(folder) {
                    this.currentFolderId = folder.id;
                    this.refreshCurrentFolder().catch((err) => {
                        this.uploadError = err && err.message ? err.message : "Failed to open folder";
                    });
                },
                openRoot() {
                    this.currentFolderId = null;
                    this.refreshCurrentFolder().catch((err) => {
                        this.uploadError = err && err.message ? err.message : "Failed to open root";
                    });
                },
                openPath(index) {
                    if (index < 0) {
                        this.openRoot();
                        return;
                    }
                    const node = this.folderPath[index];
                    if (!node) return;
                    this.currentFolderId = node.id;
                    this.refreshCurrentFolder().catch((err) => {
                        this.uploadError = err && err.message ? err.message : "Failed to open folder";
                    });
                },
                goUp() {
                    if (this.currentFolderId === null) return;
                    this.currentFolderId = this.currentParentId();
                    this.refreshCurrentFolder().catch((err) => {
                        this.uploadError = err && err.message ? err.message : "Failed to navigate up";
                    });
                },
                async renameFolder(folder) {
                    const raw = window.prompt("New folder name", folder.name);
                    if (raw === null) return;

                    const name = raw.trim();
                    if (!name || name === folder.name) return;

                    const response = await fetch("/api/folders/" + folder.id, {
                        method: "PATCH",
                        headers: { "Content-Type": "application/json" },
                        body: JSON.stringify({ name }),
                    });
                    const data = await response.json().catch(() => ({}));
                    if (!response.ok) {
                        this.uploadError = data.error || "Failed to rename folder";
                        return;
                    }

                    this.uploadError = "";
                    await Promise.all([this.refreshFolders(), this.refreshFolderPath()]);
                },
                async moveFolder(folder) {
                    const raw = window.prompt(
                        "Move folder to parent ID.\n- Leave empty to move to root\n- Example: 12",
                        folder.parent_id === null ? "" : String(folder.parent_id)
                    );
                    if (raw === null) return;

                    const trimmed = raw.trim();
                    let parentId = null;
                    if (trimmed !== "") {
                        const parsed = Number.parseInt(trimmed, 10);
                        if (!Number.isInteger(parsed) || parsed <= 0) {
                            this.uploadError = "Parent ID must be a positive number or empty for root";
                            return;
                        }
                        parentId = parsed;
                    }

                    const response = await fetch("/api/folders/" + folder.id, {
                        method: "PATCH",
                        headers: { "Content-Type": "application/json" },
                        body: JSON.stringify({ parent_id: parentId }),
                    });
                    const data = await response.json().catch(() => ({}));
                    if (!response.ok) {
                        this.uploadError = data.error || "Failed to move folder";
                        return;
                    }

                    this.uploadError = "";
                    await Promise.all([this.refreshFolders(), this.refreshFolderPath()]);
                },
                async deleteFolder(folder) {
                    if (!window.confirm("Delete folder " + folder.name + "? (folder must be empty)")) return;

                    const response = await fetch("/api/folders/" + folder.id, { method: "DELETE" });
                    const data = await response.json().catch(() => ({}));
                    if (!response.ok) {
                        this.uploadError = data.error || "Failed to delete folder";
                        return;
                    }

                    this.uploadError = "";
                    await Promise.all([this.refreshFolders(), this.refreshFolderPath()]);
                },
                onFileDragStart(file, event) {
                    this.draggingFileId = file.id;
                    if (event.dataTransfer) {
                        event.dataTransfer.effectAllowed = "move";
                        event.dataTransfer.setData("text/plain", String(file.id));
                    }
                },
                onFileDragEnd() {
                    this.draggingFileId = null;
                    this.folderDropTargetId = null;
                },
                parseDraggedFileId(event) {
                    if (this.draggingFileId !== null) return this.draggingFileId;
                    if (!event.dataTransfer) return null;
                    const raw = event.dataTransfer.getData("text/plain");
                    const parsed = Number.parseInt(raw, 10);
                    if (!Number.isInteger(parsed) || parsed <= 0) return null;
                    return parsed;
                },
                onFolderDragOver(folderId, event) {
                    const fileId = this.parseDraggedFileId(event);
                    if (fileId === null) return;
                    this.folderDropTargetId = folderId;
                    if (event.dataTransfer) {
                        event.dataTransfer.dropEffect = "move";
                    }
                },
                onFolderDragLeave(folderId) {
                    if (this.folderDropTargetId === folderId) {
                        this.folderDropTargetId = null;
                    }
                },
                async onFolderDrop(folder, event) {
                    const fileId = this.parseDraggedFileId(event);
                    this.folderDropTargetId = null;
                    this.draggingFileId = null;
                    if (fileId === null) return;
                    await this.moveFileById(fileId, folder.id);
                },
                onRootDragOver(event) {
                    const fileId = this.parseDraggedFileId(event);
                    if (fileId === null) return;
                    this.folderDropTargetId = "root";
                    if (event.dataTransfer) {
                        event.dataTransfer.dropEffect = "move";
                    }
                },
                onRootDragLeave() {
                    if (this.folderDropTargetId === "root") {
                        this.folderDropTargetId = null;
                    }
                },
                async onRootDrop(event) {
                    const fileId = this.parseDraggedFileId(event);
                    this.folderDropTargetId = null;
                    this.draggingFileId = null;
                    if (fileId === null) return;
                    await this.moveFileById(fileId, null);
                },
                async moveFileById(fileId, folderId) {
                    const response = await fetch("/api/files/" + fileId + "/move", {
                        method: "PATCH",
                        headers: { "Content-Type": "application/json" },
                        body: JSON.stringify({ folder_id: folderId }),
                    });
                    const data = await response.json().catch(() => ({}));
                    if (!response.ok) {
                        this.uploadError = data.error || "Failed to move file";
                        return;
                    }

                    this.uploadError = "";
                    this.uploadStatus = "File moved.";
                    await this.refreshCurrentFolder();
                },
                collectSelectedFiles(fileList) {
                    this.selectedFiles = Array.from(fileList || []);
                },
                onFileChange(event) {
                    this.collectSelectedFiles(event.target.files);
                },
                onDragOver() {
                    this.dragActive = true;
                },
                onDragLeave() {
                    this.dragActive = false;
                },
                onDrop(event) {
                    this.dragActive = false;
                    this.collectSelectedFiles(event.dataTransfer && event.dataTransfer.files ? event.dataTransfer.files : []);
                },
                clearSelectedFiles() {
                    this.selectedFiles = [];
                    if (this.$refs.fileInput) this.$refs.fileInput.value = "";
                },
                async createFolder() {
                    const raw = window.prompt("Folder name");
                    if (!raw) return;
                    const payload = { name: raw.trim(), parent_id: this.currentFolderId };
                    if (!payload.name) return;
                    const response = await fetch("/api/folders", {
                        method: "POST",
                        headers: { "Content-Type": "application/json" },
                        body: JSON.stringify(payload),
                    });
                    const data = await response.json().catch(() => ({}));
                    if (!response.ok) {
                        this.uploadError = data.error || "Failed to create folder";
                        return;
                    }
                    await this.refreshFolders();
                },
                async enqueueUpload(file) {
                    const body = new FormData();
                    if (this.currentFolderId !== null) {
                        body.append("folder_id", String(this.currentFolderId));
                    }
                    body.append("file", file);

                    const response = await fetch("/api/upload", { method: "POST", body });
                    const data = await response.json().catch(() => ({}));
                    if (!response.ok) {
                        if (response.status === 413) {
                            throw new Error("File is too large for current upload limit.");
                        }
                        throw new Error(data.error || `Upload failed (HTTP ${response.status})`);
                    }
                    if (!data.job_id) {
                        throw new Error("Upload queued without a valid job ID.");
                    }
                    return data.job_id;
                },
                async waitForUploadJob(jobId) {
                    const startedAt = Date.now();
                    const timeoutMs = 60 * 60 * 1000;

                    while (Date.now() - startedAt < timeoutMs) {
                        const response = await fetch("/api/jobs/" + jobId);
                        if (response.status === 401) {
                            window.location.href = "/admin/login";
                            throw new Error("Session expired. Please sign in again.");
                        }
                        const data = await response.json().catch(() => ({}));
                        if (!response.ok) {
                            throw new Error(data.error || `Job status failed (HTTP ${response.status})`);
                        }

                        if (data.status === "done") return;
                        if (data.status === "failed") {
                            throw new Error(data.error || "Upload processing failed.");
                        }

                        await new Promise((resolve) => setTimeout(resolve, 1200));
                    }

                    throw new Error("Upload processing timed out. Check status again later.");
                },
                async submitUpload() {
                    if (this.selectedFiles.length === 0) return;
                    this.uploading = true;
                    this.uploadError = "";
                    this.uploadStatus = "";
                    const total = this.selectedFiles.length;

                    try {
                        for (let i = 0; i < total; i += 1) {
                            const file = this.selectedFiles[i];
                            this.uploadStatus = `Uploading (${i + 1}/${total}): ${file.name}`;
                            const jobId = await this.enqueueUpload(file);
                            this.uploadStatus = `Processing (${i + 1}/${total}): ${file.name}`;
                            await this.waitForUploadJob(jobId);
                        }
                        await this.refreshCurrentFolder();
                        this.clearSelectedFiles();
                        this.uploadStatus = `Done: ${total} file(s) uploaded.`;
                    } catch (err) {
                        this.uploadError = err && err.message ? err.message : "Upload failed";
                        this.uploadStatus = "";
                    } finally {
                        this.uploading = false;
                    }
                },
                async deleteFile(file) {
                    if (!window.confirm("Delete " + file.original_name + "?")) return;
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
                    if (!window.confirm("Clear application log file?")) return;
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
                this.refreshCurrentFolder().catch((err) => {
                    this.uploadError = err && err.message ? err.message : "Failed to initialize dashboard";
                });
            },
        }).mount("#app");
    </script>
</body>
</html>"##;

    template
        .replace("__USERNAME__", &username_json)
        .replace("__INITIAL_FILES__", &initial_files_json)
        .replace("__INITIAL_FOLDERS__", &initial_folders_json)
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
