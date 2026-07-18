use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};

pub fn share_not_found_page() -> Response {
    (
        StatusCode::NOT_FOUND,
        Html(
            r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Share Not Found</title>
  <style>
    :root { color-scheme: light dark; }
    body {
      margin: 0;
      min-height: 100vh;
      display: grid;
      place-items: center;
      font-family: system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
      background: #f6f7fb;
      color: #111827;
    }
    main {
      width: min(520px, calc(100vw - 32px));
      text-align: center;
    }
    h1 {
      margin: 0 0 10px;
      font-size: 32px;
      font-weight: 750;
    }
    p {
      margin: 0;
      color: #5b6472;
      font-size: 16px;
      line-height: 1.5;
    }
    @media (prefers-color-scheme: dark) {
      body { background: #0f172a; color: #f8fafc; }
      p { color: #cbd5e1; }
    }
  </style>
</head>
<body>
  <main>
    <h1>Page not found</h1>
    <p>This share link does not exist or is no longer available.</p>
  </main>
</body>
</html>"#,
        ),
    )
        .into_response()
}
