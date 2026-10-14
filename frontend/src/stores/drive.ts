import { computed, onScopeDispose, ref } from 'vue'
import { defineStore } from 'pinia'
import { RealtimeClient, type RealtimeStatus } from '../realtime'
import {
  clearLogs,
  createFileShare,
  createFolder,
  deleteFilesBatch,
  deleteFolder,
  downloadUrl,
  enqueueUpload,
  getFileSummary,
  getFolderPath,
  getFolderTree,
  getMe,
  getUploadJob,
  inferFileKind,
  listFiles,
  logout,
  moveFilesBatch,
  moveFolder,
  renameFolder,
  thumbnailUrl,
  viewUrl,
} from '../api'
import type {
  FileItem,
  FileKind,
  FileShare,
  FileSummary,
  FolderItem,
  FolderPathItem,
  ToastItem,
  ToastType,
  UploadTask,
} from '../types/drive'

export type SortMode = 'newest' | 'oldest' | 'name_asc' | 'name_desc' | 'size_desc' | 'size_asc'
const UPLOAD_REQUEST_CONCURRENCY = 4
const UPLOAD_RECONCILE_INTERVAL = 15_000

interface UploadCompletedPayload {
  job_id: number
  status: 'done'
}

interface UploadFailedPayload {
  job_id: number
  status: 'failed'
  error: string
}

type UploadTerminalPayload = UploadCompletedPayload | UploadFailedPayload

function formatError(error: unknown) {
  if (error instanceof Error) return error.message
  return 'Unexpected error'
}

export const useDriveStore = defineStore('drive', () => {
  const username = ref('admin')
  const realtimeStatus = ref<RealtimeStatus>('idle')

  const folders = ref<FolderItem[]>([])
  const folderPath = ref<FolderPathItem[]>([])
  const currentFolderId = ref<number | null>(null)

  const files = ref<FileItem[]>([])
  const fileSummary = ref<FileSummary>({
    total_files: 0,
    total_original_size: 0,
    total_stored_size: 0,
  })
  const nextCursor = ref<number | null>(null)
  const hasMore = ref(false)

  const loadingFolders = ref(false)
  const loadingFiles = ref(false)
  const loadingMore = ref(false)
  const busyAction = ref(false)
  const sharingFileId = ref<number | null>(null)

  const selectedIds = ref<number[]>([])
  const selectingAllFiles = ref(false)
  const allFilesSelected = ref(false)
  const activeFileId = ref<number | null>(null)
  const draggedFileId = ref<number | null>(null)
  const dropFolderId = ref<number | null>(null)

  const search = ref('')
  const typeFilter = ref<FileKind | 'all'>('all')
  const sortMode = ref<SortMode>('newest')
  const listMode = ref<'grid' | 'list'>('grid')

  const uploadTasks = ref<UploadTask[]>([])
  const uploadTaskCleanupTimers = new Map<string, ReturnType<typeof setTimeout>>()
  const uploadTerminalEvents = new Map<number, UploadTerminalPayload>()
  const uploadEventWaiters = new Map<number, Set<() => void>>()
  const activeUploadReconciliations = new Set<number>()
  const toasts = ref<ToastItem[]>([])
  const toastTimers = new Map<number, ReturnType<typeof setTimeout>>()
  let toastIdSeq = 0
  let realtimeInitialized = false
  let driveRefreshTimer: ReturnType<typeof setTimeout> | null = null
  const realtime = new RealtimeClient()

  const folderMap = computed(() => new Map(folders.value.map((folder) => [folder.id, folder])))

  const rootFolders = computed(() =>
    folders.value
      .filter((folder) => folder.parent_id === null)
      .sort((a, b) => a.name.localeCompare(b.name)),
  )

  const filesById = computed(() => new Map(files.value.map((file) => [file.id, file])))

  const activeFile = computed(() => (activeFileId.value ? filesById.value.get(activeFileId.value) || null : null))

  const filteredFiles = computed(() => {
    const query = search.value.trim().toLowerCase()

    const result = files.value.filter((file) => {
      if (query && !file.original_name.toLowerCase().includes(query)) return false
      if (typeFilter.value !== 'all' && inferFileKind(file.original_name) !== typeFilter.value) return false
      return true
    })

    result.sort((a, b) => {
      switch (sortMode.value) {
        case 'oldest':
          return a.id - b.id
        case 'name_asc':
          return a.original_name.localeCompare(b.original_name)
        case 'name_desc':
          return b.original_name.localeCompare(a.original_name)
        case 'size_desc':
          return b.compressed_size - a.compressed_size
        case 'size_asc':
          return a.compressed_size - b.compressed_size
        case 'newest':
        default:
          return b.id - a.id
      }
    })

    return result
  })

  const selectedFiles = computed(() =>
    selectedIds.value
      .map((id) => filesById.value.get(id))
      .filter((item): item is FileItem => Boolean(item)),
  )

  const allFilteredSelected = computed(() => {
    if (filteredFiles.value.length === 0) return false
    const idSet = new Set(selectedIds.value)
    return filteredFiles.value.every((file) => idSet.has(file.id))
  })

  const currentFolderLabel = computed(() => {
    if (currentFolderId.value === null) return 'Root'
    return folderMap.value.get(currentFolderId.value)?.name || 'Folder'
  })

  const totalVisibleOriginal = computed(() =>
    filteredFiles.value.reduce((sum, file) => sum + (Number(file.original_size) || 0), 0),
  )
  const totalVisibleStored = computed(() =>
    filteredFiles.value.reduce((sum, file) => sum + (Number(file.compressed_size) || 0), 0),
  )

  const lightboxOpen = ref(false)

  const lightboxFiles = computed(() =>
    filteredFiles.value.filter((file) => {
      const kind = inferFileKind(file.original_name)
      return kind === 'image' || kind === 'video'
    }),
  )

  const lightboxIndex = computed(() =>
    lightboxFiles.value.findIndex((file) => file.id === activeFileId.value),
  )

  const lightboxFile = computed(() => {
    const index = lightboxIndex.value
    return index >= 0 ? lightboxFiles.value[index] : null
  })

  function dismissToast(id: number) {
    const timer = toastTimers.get(id)
    if (timer) {
      clearTimeout(timer)
      toastTimers.delete(id)
    }
    toasts.value = toasts.value.filter((item) => item.id !== id)
  }

  function showToast(type: ToastType, message: string, title?: string, timeoutMs?: number) {
    const id = ++toastIdSeq
    const computedTimeout = timeoutMs ?? (type === 'error' ? 5200 : 3600)
    const toast: ToastItem = {
      id,
      type,
      title: title ?? (type === 'error' ? 'Error' : type === 'ok' ? 'Success' : 'Notice'),
      message,
      timeoutMs: computedTimeout,
    }

    toasts.value = [...toasts.value, toast]
    const timer = setTimeout(() => dismissToast(id), computedTimeout)
    toastTimers.set(id, timer)
  }

  function scheduleUploadTaskCleanup(localId: string, delayMs = 5000) {
    const existing = uploadTaskCleanupTimers.get(localId)
    if (existing) {
      clearTimeout(existing)
    }
    const timer = setTimeout(() => {
      uploadTasks.value = uploadTasks.value.filter((task) => task.localId !== localId)
      uploadTaskCleanupTimers.delete(localId)
    }, delayMs)
    uploadTaskCleanupTimers.set(localId, timer)
  }

  onScopeDispose(() => {
    realtime.disconnect()
    if (driveRefreshTimer) clearTimeout(driveRefreshTimer)
    for (const timer of toastTimers.values()) {
      clearTimeout(timer)
    }
    toastTimers.clear()

    for (const timer of uploadTaskCleanupTimers.values()) {
      clearTimeout(timer)
    }
    uploadTaskCleanupTimers.clear()
  })

  function scheduleDriveRefresh() {
    if (driveRefreshTimer) clearTimeout(driveRefreshTimer)
    driveRefreshTimer = setTimeout(() => {
      driveRefreshTimer = null
      void Promise.all([loadFiles(true), refreshFolders(), refreshSummary()]).catch((error) => {
        console.error('Failed to reconcile realtime drive state', error)
      })
    }, 180)
  }

  function wakeUploadWaiters(jobId: number) {
    for (const resolve of uploadEventWaiters.get(jobId) ?? []) resolve()
    uploadEventWaiters.delete(jobId)
  }

  function applyUploadTerminalEvent(payload: UploadTerminalPayload) {
    const task = uploadTasks.value.find((candidate) => candidate.jobId === payload.job_id)
    if (!task) {
      uploadTerminalEvents.set(payload.job_id, payload)
      setTimeout(() => uploadTerminalEvents.delete(payload.job_id), 60_000)
      scheduleDriveRefresh()
      return
    }

    if (payload.status === 'done') {
      task.status = 'done'
      scheduleUploadTaskCleanup(task.localId, 4200)
    } else {
      task.status = 'failed'
      task.error = payload.error || 'Processing failed'
      scheduleUploadTaskCleanup(task.localId, 9000)
    }
    wakeUploadWaiters(payload.job_id)
    scheduleDriveRefresh()
  }

  function waitForUploadEvent(jobId: number, timeoutMs: number) {
    return new Promise<void>((resolve) => {
      const finish = () => {
        clearTimeout(timeout)
        uploadEventWaiters.get(jobId)?.delete(finish)
        resolve()
      }
      const timeout = setTimeout(finish, timeoutMs)
      const waiters = uploadEventWaiters.get(jobId) ?? new Set<() => void>()
      waiters.add(finish)
      uploadEventWaiters.set(jobId, waiters)
    })
  }

  async function reconcileActiveUploads() {
    const activeTasks = uploadTasks.value.filter(
      (task) => task.jobId && task.status === 'processing',
    )
    await Promise.allSettled(activeTasks.map((task) => reconcileUploadTask(task)))
  }

  function initializeRealtime() {
    if (realtimeInitialized) return
    realtimeInitialized = true

    realtime.onStatus((status) => {
      realtimeStatus.value = status
    })
    realtime.on<UploadCompletedPayload>('upload.completed', ({ payload }) => {
      applyUploadTerminalEvent(payload)
    })
    realtime.on<UploadFailedPayload>('upload.failed', ({ payload }) => {
      applyUploadTerminalEvent(payload)
    })

    for (const event of [
      'file.created',
      'file.moved',
      'file.deleted',
      'folder.created',
      'folder.renamed',
      'folder.moved',
      'folder.deleted',
    ]) {
      realtime.on(event, scheduleDriveRefresh)
    }

    let connectedBefore = false
    realtime.on('connection.established', () => {
      if (connectedBefore) {
        void reconcileActiveUploads()
        scheduleDriveRefresh()
      }
      connectedBefore = true
    })
    realtime.connect()
  }

  function clearSelection() {
    selectedIds.value = []
    allFilesSelected.value = false
  }

  function toggleSelection(id: number) {
    allFilesSelected.value = false
    if (selectedIds.value.includes(id)) {
      selectedIds.value = selectedIds.value.filter((item) => item !== id)
    } else {
      selectedIds.value = [...selectedIds.value, id]
    }
  }

  async function toggleSelectAllFiles() {
    if (selectingAllFiles.value) return
    selectingAllFiles.value = true
    const folderId = currentFolderId.value
    const query = search.value.trim().toLowerCase()
    const kindFilter = typeFilter.value
    const matchingIds: number[] = []
    let cursor: number | null = null

    try {
      let hasMorePages = true
      while (hasMorePages) {
        const page = await listFiles(folderId, cursor, 100)
        matchingIds.push(...page.items
          .filter((file) => {
            if (query && !file.original_name.toLowerCase().includes(query)) return false
            if (kindFilter !== 'all' && inferFileKind(file.original_name) !== kindFilter) return false
            return true
          })
          .map((file) => file.id))
        cursor = page.next_cursor
        hasMorePages = page.has_more
      }

      const selected = new Set(selectedIds.value)
      const allMatchingSelected = matchingIds.length > 0 && matchingIds.every((id) => selected.has(id))
      if (allMatchingSelected) {
        const matching = new Set(matchingIds)
        selectedIds.value = selectedIds.value.filter((id) => !matching.has(id))
        allFilesSelected.value = false
      } else {
        for (const id of matchingIds) selected.add(id)
        selectedIds.value = [...selected]
        allFilesSelected.value = matchingIds.length > 0
      }
    } catch (error) {
      showToast('error', formatError(error))
    } finally {
      selectingAllFiles.value = false
    }
  }

  function setActiveFile(id: number | null) {
    activeFileId.value = id
  }

  function openLightbox(fileId: number) {
    activeFileId.value = fileId
    lightboxOpen.value = true
  }

  function closeLightbox() {
    lightboxOpen.value = false
  }

  function lightboxNext() {
    if (lightboxFiles.value.length === 0) return
    const index = lightboxIndex.value
    if (index < 0) {
      const first = lightboxFiles.value[0]
      if (first) activeFileId.value = first.id
      return
    }
    const next = (index + 1) % lightboxFiles.value.length
    const nextFile = lightboxFiles.value[next]
    if (nextFile) activeFileId.value = nextFile.id
  }

  function lightboxPrev() {
    if (lightboxFiles.value.length === 0) return
    const index = lightboxIndex.value
    if (index < 0) {
      const first = lightboxFiles.value[0]
      if (first) activeFileId.value = first.id
      return
    }
    const prev = (index - 1 + lightboxFiles.value.length) % lightboxFiles.value.length
    const prevFile = lightboxFiles.value[prev]
    if (prevFile) activeFileId.value = prevFile.id
  }

  async function refreshMe() {
    const me = await getMe()
    username.value = me.username
  }

  async function refreshFolders() {
    loadingFolders.value = true
    try {
      folders.value = await getFolderTree()
    } finally {
      loadingFolders.value = false
    }
  }

  async function refreshFolderPath() {
    folderPath.value = await getFolderPath(currentFolderId.value)
  }

  async function refreshSummary() {
    fileSummary.value = await getFileSummary()
  }

  async function loadFiles(reset = false) {
    if (reset) {
      loadingFiles.value = true
      files.value = []
      nextCursor.value = null
      hasMore.value = false
      clearSelection()
    } else {
      loadingMore.value = true
    }

    try {
      const response = await listFiles(currentFolderId.value, reset ? null : nextCursor.value, 30)
      if (reset) {
        files.value = response.items
      } else {
        files.value = [...files.value, ...response.items]
      }
      nextCursor.value = response.next_cursor
      hasMore.value = response.has_more

      if (activeFileId.value !== null && !files.value.some((file) => file.id === activeFileId.value)) {
        activeFileId.value = null
      }
    } finally {
      loadingFiles.value = false
      loadingMore.value = false
    }
  }

  async function openFolder(folderId: number | null) {
    currentFolderId.value = folderId
    await Promise.all([loadFiles(true), refreshFolderPath()])
  }

  async function initialize() {
    initializeRealtime()
    await Promise.all([refreshMe(), refreshFolders(), refreshSummary()])
    await openFolder(null)
  }

  async function loadMore() {
    if (!hasMore.value || loadingMore.value) return
    await loadFiles(false)
  }

  function canDropIntoFolder(folderId: number | null) {
    return typeof folderId === 'number' || folderId === null
  }

  async function moveSelectedTo(folderId: number | null, explicitIds?: number[]) {
    const ids = explicitIds?.length ? explicitIds : selectedIds.value
    const unique = [...new Set(ids)].filter((id) => id > 0)
    if (unique.length === 0) return

    busyAction.value = true
    try {
      const result = await moveFilesBatch(unique, folderId)
      if (result.failed.length > 0) {
        showToast('error', `Moved ${result.moved_ids.length}, failed ${result.failed.length}`)
      } else {
        showToast('ok', `Moved ${result.moved_ids.length} file(s)`)
      }
      await Promise.all([loadFiles(true), refreshFolders(), refreshSummary()])
    } catch (error) {
      showToast('error', formatError(error))
    } finally {
      busyAction.value = false
      dropFolderId.value = null
    }
  }

  async function deleteSelectedIds(ids: number[]) {
    const unique = [...new Set(ids)].filter((id) => id > 0)
    if (unique.length === 0) return false

    busyAction.value = true
    try {
      const result = await deleteFilesBatch(unique)
      if (result.failed.length > 0) {
        showToast('error', `Deleted ${result.deleted_ids.length}, failed ${result.failed.length}`)
      } else {
        showToast('ok', `Deleted ${result.deleted_ids.length} file(s)`)
      }
      await Promise.all([loadFiles(true), refreshFolders(), refreshSummary()])
      return result.failed.length === 0
    } catch (error) {
      showToast('error', formatError(error))
      return false
    } finally {
      busyAction.value = false
    }
  }

  async function createShareForFile(fileId: number, expiresInHours: number): Promise<FileShare | null> {
    sharingFileId.value = fileId
    try {
      const share = await createFileShare(fileId, expiresInHours)
      showToast('ok', 'Share link created')
      return {
        ...share,
        url: new URL(share.url, window.location.origin).toString(),
      }
    } catch (error) {
      showToast('error', formatError(error))
      return null
    } finally {
      sharingFileId.value = null
    }
  }

  async function createFolderWithName(name: string) {
    busyAction.value = true
    try {
      await createFolder(name, currentFolderId.value)
      showToast('ok', 'Folder created')
      await refreshFolders()
      return true
    } catch (error) {
      showToast('error', formatError(error))
      return false
    } finally {
      busyAction.value = false
    }
  }

  async function renameCurrentFolderWithName(name: string) {
    if (currentFolderId.value === null) return
    const folder = folderMap.value.get(currentFolderId.value)
    if (!folder) return
    if (!name || name === folder.name) return

    busyAction.value = true
    try {
      await renameFolder(folder.id, name)
      showToast('ok', 'Folder renamed')
      await Promise.all([refreshFolders(), refreshFolderPath()])
      return true
    } catch (error) {
      showToast('error', formatError(error))
      return false
    } finally {
      busyAction.value = false
    }
  }

  async function moveCurrentFolderTo(parentId: number | null) {
    if (currentFolderId.value === null) return
    const folder = folderMap.value.get(currentFolderId.value)
    if (!folder) return

    busyAction.value = true
    try {
      await moveFolder(folder.id, parentId)
      showToast('ok', 'Folder moved')
      await Promise.all([refreshFolders(), refreshFolderPath()])
      return true
    } catch (error) {
      showToast('error', formatError(error))
      return false
    } finally {
      busyAction.value = false
    }
  }

  async function deleteCurrentFolderConfirmed() {
    if (currentFolderId.value === null) return
    const folder = folderMap.value.get(currentFolderId.value)
    if (!folder) return

    busyAction.value = true
    try {
      await deleteFolder(folder.id)
      showToast('ok', 'Folder deleted')
      await Promise.all([refreshFolders(), openFolder(folder.parent_id)])
      return true
    } catch (error) {
      showToast('error', formatError(error))
      return false
    } finally {
      busyAction.value = false
    }
  }

  async function runWithConcurrency<T>(
    items: T[],
    limit: number,
    worker: (item: T) => Promise<void>,
  ) {
    const queue = [...items]
    const runnerCount = Math.max(1, Math.min(limit, queue.length))
    const runners = Array.from({ length: runnerCount }, async () => {
      while (queue.length > 0) {
        const item = queue.shift()
        if (!item) return
        await worker(item)
      }
    })
    await Promise.all(runners)
  }

  async function reconcileUploadTask(task: UploadTask) {
    const jobId = task.jobId
    if (!jobId || task.status !== 'processing' || activeUploadReconciliations.has(jobId)) return
    activeUploadReconciliations.add(jobId)

    try {
      const earlyEvent = uploadTerminalEvents.get(jobId)
      if (earlyEvent) {
        uploadTerminalEvents.delete(jobId)
        applyUploadTerminalEvent(earlyEvent)
        return
      }

      const startedAt = Date.now()
      while (task.status === 'processing' && Date.now() - startedAt < 60 * 60 * 1000) {
        // WebSocket delivery is the fast path. HTTP is only used periodically to
        // recover an event lost during a disconnect or a full client queue.
        const eventWait = waitForUploadEvent(jobId, UPLOAD_RECONCILE_INTERVAL)
        // Close the race where the terminal event lands between evaluating the
        // loop condition and registering this waiter.
        if (task.status !== 'processing') wakeUploadWaiters(jobId)
        await eventWait
        if (task.status !== 'processing') return

        let job
        try {
          job = await getUploadJob(jobId)
        } catch (error) {
          console.warn(`Failed to reconcile upload job ${jobId}`, error)
          continue
        }
        if (job.status === 'done') {
          applyUploadTerminalEvent({ job_id: jobId, status: 'done' })
          return
        }
        if (job.status === 'failed') {
          applyUploadTerminalEvent({
            job_id: jobId,
            status: 'failed',
            error: job.error || 'Processing failed',
          })
          return
        }
      }

      if (task.status === 'processing') {
        task.status = 'failed'
        task.error = 'Upload processing timeout'
        scheduleUploadTaskCleanup(task.localId, 9000)
      }
    } finally {
      activeUploadReconciliations.delete(jobId)
    }
  }

  async function uploadFiles(fileList: FileList | null) {
    if (!fileList || fileList.length === 0) return

    const filesToUpload = [...fileList]
    const queuedJobs: UploadTask[] = []

    await runWithConcurrency(filesToUpload, UPLOAD_REQUEST_CONCURRENCY, async (file) => {
      const localId = `${Date.now()}-${Math.random()}`
      const task: UploadTask = { localId, name: file.name, status: 'queued' }
      uploadTasks.value = [task, ...uploadTasks.value]

      try {
        task.status = 'uploading'
        const queued = await enqueueUpload(file, currentFolderId.value)
        task.jobId = queued.job_id
        task.status = 'processing'
        queuedJobs.push(task)

        const earlyEvent = uploadTerminalEvents.get(queued.job_id)
        if (earlyEvent) {
          uploadTerminalEvents.delete(queued.job_id)
          applyUploadTerminalEvent(earlyEvent)
        }
      } catch (error) {
        task.status = 'failed'
        task.error = formatError(error)
        scheduleUploadTaskCleanup(task.localId, 9000)
      }
    })

    await Promise.allSettled(
      queuedJobs.map((task) => reconcileUploadTask(task)),
    )

    await Promise.all([loadFiles(true), refreshFolders(), refreshSummary()])
  }

  async function doClearLogs() {
    try {
      await clearLogs()
      showToast('ok', 'Logs cleared')
    } catch (error) {
      showToast('error', formatError(error))
    }
  }

  async function doLogout() {
    realtime.disconnect()
    await logout()
    window.location.href = '/admin/login'
  }

  function fileKind(file: FileItem): FileKind {
    return inferFileKind(file.original_name)
  }

  function fileViewUrl(file: FileItem) {
    return viewUrl(file.id)
  }

  function fileDownloadUrl(file: FileItem) {
    return downloadUrl(file.id)
  }

  function fileThumbUrl(file: FileItem) {
    return thumbnailUrl(file.id)
  }

  return {
    username,
    realtimeStatus,
    folders,
    folderMap,
    rootFolders,
    folderPath,
    currentFolderId,
    currentFolderLabel,

    files,
    filteredFiles,
    fileSummary,
    hasMore,
    loadingFolders,
    loadingFiles,
    loadingMore,
    busyAction,
    sharingFileId,

    selectedIds,
    selectingAllFiles,
    allFilesSelected,
    selectedFiles,
    allFilteredSelected,
    activeFile,
    activeFileId,
    draggedFileId,
    dropFolderId,

    search,
    typeFilter,
    sortMode,
    listMode,

    totalVisibleOriginal,
    totalVisibleStored,

    lightboxOpen,
    lightboxFiles,
    lightboxFile,

    uploadTasks,
    toasts,

    setActiveFile,
    clearSelection,
    toggleSelection,
    toggleSelectAllFiles,
    openLightbox,
    closeLightbox,
    lightboxNext,
    lightboxPrev,
    canDropIntoFolder,

    refreshFolders,
    refreshSummary,
    loadFiles,
    loadMore,
    openFolder,
    initialize,

    moveSelectedTo,
    deleteSelectedIds,
    createShareForFile,

    createFolderWithName,
    renameCurrentFolderWithName,
    moveCurrentFolderTo,
    deleteCurrentFolderConfirmed,

    uploadFiles,
    doClearLogs,
    doLogout,

    fileKind,
    fileViewUrl,
    fileDownloadUrl,
    fileThumbUrl,
    dismissToast,
    showToast,
  }
})
