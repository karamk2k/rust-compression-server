# R2 Setup (Cloudflare)

Use this when you want file storage in Cloudflare R2 instead of local disk.

## 1) Required `.env` values

```env
STORAGE_BACKEND=r2
R2_ENDPOINT=https://a9412170bd89a4ea1ba32dd4836b1a1a.r2.cloudflarestorage.com
R2_BUCKET=your_bucket_name
R2_REGION=auto
R2_ACCESS_KEY_ID=your_access_key_id
R2_SECRET_ACCESS_KEY=your_secret_key
R2_KEY_PREFIX=uploads
```

Keep `R2_SECRET_ACCESS_KEY` private.

## 2) Behavior in app

- Upload flow keeps current compression/transcode logic.
- After processing, files are uploaded to R2.
- DB stores object paths as `r2://<key>`.
- `View` streams uncompressed files from R2.
- `Download` works from R2 too.

## 3) Fallback to local storage

Set:

```env
STORAGE_BACKEND=local
```
