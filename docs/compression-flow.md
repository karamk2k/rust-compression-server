# Compression Flow (Function Map)

This file explains how compression works in this project using the real Rust function names and signatures.

## 1) API Upload Path (`POST /api/upload`)

Entry point:
- `src/routes/api_routes.rs`
- `async fn upload_file(State(state): State<AppState>, headers: HeaderMap, mut multipart: Multipart) -> Response`

Main call:
- `FileService::upload_and_compress(...)`

## 2) Core Service: Upload + Smart Compression

Main function:
- `src/services/file_service.rs`
- `pub async fn upload_and_compress(&self, file_name: Option<&str>, bytes: &[u8], uploaded_by: i64) -> Result<UploadResult>`

Flow inside this function:
1. Sanitize file name:
   - `fn sanitize_filename(name: &str) -> String`
2. Detect extension:
   - `fn file_extension(file_name: &str) -> String`
3. Optional media transcode (Phase 2):
   - `MediaTranscodeService::transcode_if_smaller(&self, input_path: &Path, ext: &str) -> Result<Option<u64>>`
   - If enabled and ffmpeg succeeds with smaller output:
     - replace file in `stored_path`
     - update current size baseline
   - If ffmpeg missing, fails, or output is larger:
     - keep original upload
4. Decide whether zstd should run:
   - `fn should_try_zstd(ext: &str) -> bool`
5. If extension is allowed for zstd:
   - Run blocking compression:
     - `FileCompressor::compress_file(&self, input_path: &str, output_path: &str) -> std::io::Result<()>`
   - Compare `candidate_size` vs current size baseline
6. Smart keep/discard for zstd:
   - If compressed is smaller:
     - keep `.zst`
     - set `is_compressed = true`
     - background delete original:
       - `pub async fn delete_original_file(&self, file_name: &str) -> Result<()>`
       - `async fn delete_by_path(&self, file_path: &str) -> Result<()>`
   - If compressed is not smaller:
     - delete candidate `.zst`
     - keep original path
     - set `is_compressed = false`
7. Persist metadata:
   - `db::insert_file(...)`
8. Return:
   - `UploadResult`

## 3) File Compressor Functions

File:
- `src/file_compressor.rs`

Used by API flow:
- `pub fn compress_file(&self, input_path: &str, output_path: &str) -> std::io::Result<()>`
- `pub fn decompress_file_to_bytes(&self, input_path: &str) -> std::io::Result<Vec<u8>>`

## 4) View Path (`GET /api/files/:id/view`)

Entry point:
- `src/routes/api_routes.rs`
- `async fn view_file(Path(id): Path<i64>, State(state): State<AppState>, headers: HeaderMap) -> Response`

Service call:
- `pub async fn view_file_bytes(&self, file_id: i64) -> Result<Option<(String, Vec<u8>)>>`

Behavior:
- If `is_compressed == true`: decompress from `compressed_path` in memory.
- If `is_compressed == false`: stream file directly from `stored_path` (no full in-memory read).
- Supports `Range` requests for uncompressed files (`206 Partial Content`) for video seek and large-file viewing.

## 5) Download Path (`GET /api/files/:id/download`)

Behavior:
- Always returns `Content-Disposition: attachment`.
- If `is_compressed == true`: decompresses first and downloads original content.
- If `is_compressed == false`: streams file directly from disk.

## 6) Delete Path (`DELETE /api/files/:id`)

Entry point:
- `src/routes/api_routes.rs`
- `async fn delete_file(Path(id): Path<i64>, State(state): State<AppState>, headers: HeaderMap) -> Response`

Service call:
- `pub async fn delete_file_by_id(&self, file_id: i64) -> Result<bool>`

Behavior:
1. Read DB row.
2. Delete `stored_path`.
3. Delete `compressed_path` only if it is different from `stored_path`.
4. Remove DB row with `db::delete_file_by_id(...)`.

## 7) Folder Watcher Path (Separate From API)

File:
- `src/folder_watcher.rs`

Main functions:
- `pub fn watch(&self) -> notify::Result<()>`
- `fn compress_and_replace(&self, category: &str, file_path: &Path)`

Behavior:
- Always compress to `.zst`.
- Deletes original file.
- Does not currently use the API smart keep/discard rule.

## 8) DB Fields For Compression State

Model:
- `src/models/file_model.rs` -> `FileRecord`

Key fields:
- `is_compressed: bool`
- `original_size: i64`
- `compressed_size: i64`
- `stored_path: String`
- `compressed_path: String`

Migration:
- `migrations/202603070003_add_is_compressed_to_files.sql`

## 9) Why JPG/MP4 Usually Do Not Shrink

Files like `jpg`, `png`, `mp4`, `webm`, `zip`, `rar` are already compressed formats.
This project skips zstd for those extensions in `should_try_zstd(...)` to avoid wasting CPU and disk.

Phase 2 adds ffmpeg transcoding for selected media extensions first, then zstd logic can still run for non-media files.

## 10) Phase 2 Environment Variables

Set in `.env`:
- `ENABLE_MEDIA_TRANSCODE=true`
- `FFMPEG_BIN=ffmpeg`
- `FFMPEG_VIDEO_CRF=28`
- `FFMPEG_VIDEO_PRESET=medium`
- `FFMPEG_JPEG_QUALITY=4`
- `FFMPEG_WEBP_QUALITY=75`

## 11) Storage Backend (Local or R2)

Config in `.env`:
- `STORAGE_BACKEND=local` or `STORAGE_BACKEND=r2`
- `R2_ENDPOINT=...`
- `R2_BUCKET=...`
- `R2_REGION=auto`
- `R2_ACCESS_KEY_ID=...`
- `R2_SECRET_ACCESS_KEY=...`
- `R2_KEY_PREFIX=uploads`

When backend is `r2`:
- Uploaded files are processed locally first (transcode/zstd), then uploaded to R2.
- DB paths are saved as `r2://<key>`.
- View/download for uncompressed files stream from R2.
- Compressed files are downloaded and decompressed by app before response.
