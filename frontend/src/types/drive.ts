export interface MeResponse {
  id: number
  username: string
}

export interface FolderItem {
  id: number
  name: string
  parent_id: number | null
  created_by: number
  created_at: string
  file_count: number
}

export interface FolderPathItem {
  id: number
  name: string
  parent_id: number | null
  created_by: number
  created_at: string
}

export interface FileItem {
  id: number
  original_name: string
  folder_id: number | null
  stored_path: string
  compressed_path: string
  is_compressed: boolean
  original_size: number
  compressed_size: number
  uploaded_by: number
  created_at: string
}

export interface FileSummary {
  total_files: number
  total_original_size: number
  total_stored_size: number
}

export interface PaginatedFilesResponse {
  items: FileItem[]
  next_cursor: number | null
  has_more: boolean
}

export interface UploadJobStatus {
  id: number
  original_name: string
  status: 'pending' | 'processing' | 'done' | 'failed'
  file_id: number | null
  error: string | null
  created_at: string
  started_at: string | null
  finished_at: string | null
  updated_at: string
}

export type FileKind = 'image' | 'video' | 'doc' | 'audio' | 'other'

export interface UploadTask {
  localId: string
  name: string
  status: 'queued' | 'uploading' | 'processing' | 'done' | 'failed'
  error?: string
}

export type ToastType = 'ok' | 'error' | 'info'

export interface ToastItem {
  id: number
  type: ToastType
  title: string
  message: string
  timeoutMs: number
}
