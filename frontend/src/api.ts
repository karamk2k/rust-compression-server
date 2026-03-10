import type {
  FileSummary,
  FolderItem,
  FolderPathItem,
  MeResponse,
  PaginatedFilesResponse,
  UploadJobStatus,
} from './types/drive'

const JSON_HEADERS = {
  'Content-Type': 'application/json',
}

async function request<T>(input: string, init?: RequestInit): Promise<T> {
  const response = await fetch(input, init)
  if (response.status === 401) {
    window.location.href = '/admin/login'
    throw new Error('Unauthorized')
  }

  const data = await response.json().catch(() => ({}))
  if (!response.ok) {
    const message = Array.isArray(data.error)
      ? data.error.join('; ')
      : data.error || `Request failed (${response.status})`
    throw new Error(message)
  }

  return data as T
}

export async function getMe() {
  return request<MeResponse>('/api/me')
}

export async function getFolderTree() {
  return request<FolderItem[]>('/api/folders/tree')
}

export async function getFolderPath(folderId: number | null) {
  if (folderId === null) return []
  return request<FolderPathItem[]>(`/api/folders/path?folder_id=${folderId}`)
}

export async function listFiles(folderId: number | null, cursor: number | null, limit = 60) {
  const params = new URLSearchParams()
  params.set('limit', String(limit))
  if (folderId !== null) params.set('folder_id', String(folderId))
  if (cursor !== null) params.set('cursor', String(cursor))
  return request<PaginatedFilesResponse>(`/api/files?${params.toString()}`)
}

export async function getFileSummary() {
  return request<FileSummary>('/api/files/summary')
}

export async function moveFilesBatch(ids: number[], folderId: number | null) {
  return request<{ moved_ids: number[]; failed: Array<{ id: number; error: string }> }>(
    '/api/files/move-batch',
    {
      method: 'PATCH',
      headers: JSON_HEADERS,
      body: JSON.stringify({ ids, folder_id: folderId }),
    },
  )
}

export async function deleteFilesBatch(ids: number[]) {
  return request<{ deleted_ids: number[]; failed: Array<{ id: number; error: string }> }>(
    '/api/files/delete-batch',
    {
      method: 'POST',
      headers: JSON_HEADERS,
      body: JSON.stringify({ ids }),
    },
  )
}

export async function createFolder(name: string, parentId: number | null) {
  return request<FolderItem>('/api/folders', {
    method: 'POST',
    headers: JSON_HEADERS,
    body: JSON.stringify({ name, parent_id: parentId }),
  })
}

export async function renameFolder(folderId: number, name: string) {
  return request<FolderItem>(`/api/folders/${folderId}`, {
    method: 'PATCH',
    headers: JSON_HEADERS,
    body: JSON.stringify({ name }),
  })
}

export async function moveFolder(folderId: number, parentId: number | null) {
  return request<FolderItem>(`/api/folders/${folderId}`, {
    method: 'PATCH',
    headers: JSON_HEADERS,
    body: JSON.stringify({ parent_id: parentId }),
  })
}

export async function deleteFolder(folderId: number) {
  return request<{ deleted: boolean; id: number }>(`/api/folders/${folderId}`, {
    method: 'DELETE',
  })
}

export async function enqueueUpload(file: File, folderId: number | null) {
  const body = new FormData()
  if (folderId !== null) body.append('folder_id', String(folderId))
  body.append('file', file)

  return request<{ job_id: number; status: string }>('/api/upload', {
    method: 'POST',
    body,
  })
}

export async function getUploadJob(jobId: number) {
  return request<UploadJobStatus>(`/api/jobs/${jobId}`)
}

export async function clearLogs() {
  return request<{ cleared: boolean }>('/admin/logs/clear', { method: 'POST' })
}

export async function logout() {
  await fetch('/admin/logout', { method: 'POST' })
}

export function viewUrl(fileId: number) {
  return `/api/files/${fileId}/view`
}

export function downloadUrl(fileId: number) {
  return `/api/files/${fileId}/download`
}

export function thumbnailUrl(fileId: number) {
  return `/api/files/${fileId}/thumb`
}

export function inferFileKind(fileName: string): 'image' | 'video' | 'doc' | 'audio' | 'other' {
  const ext = fileName.split('.').pop()?.toLowerCase() || ''
  if (['jpg', 'jpeg', 'png', 'gif', 'webp', 'bmp', 'svg'].includes(ext)) return 'image'
  if (['mp4', 'webm', 'mov', 'avi', 'mkv'].includes(ext)) return 'video'
  if (['mp3', 'wav', 'flac', 'ogg'].includes(ext)) return 'audio'
  if (['pdf', 'doc', 'docx', 'txt', 'md', 'json', 'csv', 'xls', 'xlsx', 'ppt', 'pptx'].includes(ext)) return 'doc'
  return 'other'
}
