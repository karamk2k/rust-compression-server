<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref } from 'vue'
import FolderTreeNode from './components/FolderTreeNode.vue'
import { useDriveStore } from './stores/drive'
import type { FileItem } from './types/drive'

interface TreeFolder {
  id: number
  name: string
  file_count: number
  children: TreeFolder[]
}

const store = useDriveStore()
const uploadInput = ref<HTMLInputElement | null>(null)
const moveTarget = ref<string>('')
const folderModalOpen = ref(false)
const folderNameDraft = ref('')
const folderNameError = ref('')
const folderNameInput = ref<HTMLInputElement | null>(null)
const renameModalOpen = ref(false)
const renameFolderDraft = ref('')
const renameFolderError = ref('')
const renameFolderInput = ref<HTMLInputElement | null>(null)
const moveModalOpen = ref(false)
const moveFolderDraft = ref('')
const moveFolderError = ref('')
const moveFolderInput = ref<HTMLInputElement | null>(null)
const confirmModalOpen = ref(false)
const confirmModalTitle = ref('Confirm Action')
const confirmModalMessage = ref('')
const confirmModalActionLabel = ref('Confirm')
const confirmModalDanger = ref(false)
let confirmModalAction: (() => Promise<boolean | void>) | null = null
const uploadDragDepth = ref(0)
const uploadDragActive = ref(false)

const folderTree = computed<TreeFolder[]>(() => {
  const byParent = new Map<number | null, TreeFolder[]>()
  for (const folder of store.folders) {
    const list = byParent.get(folder.parent_id) || []
    list.push({
      id: folder.id,
      name: folder.name,
      file_count: folder.file_count,
      children: [],
    })
    byParent.set(folder.parent_id, list)
  }

  for (const [parentId, children] of byParent.entries()) {
    if (parentId === null) continue
    const parentGroup = [...(byParent.get(null) || []), ...store.folders.map((f) => ({
      id: f.id,
      name: f.name,
      file_count: f.file_count,
      children: [],
    }))]
    const parent = parentGroup.find((node) => node.id === parentId)
    if (parent) parent.children = children.sort((a, b) => a.name.localeCompare(b.name))
  }

  const map = new Map<number, TreeFolder>()
  for (const folder of store.folders) {
    map.set(folder.id, {
      id: folder.id,
      name: folder.name,
      file_count: folder.file_count,
      children: [],
    })
  }
  for (const folder of store.folders) {
    const node = map.get(folder.id)
    if (!node) continue
    if (folder.parent_id !== null) {
      const parent = map.get(folder.parent_id)
      if (parent) parent.children.push(node)
    }
  }

  const roots = [...map.values()]
    .filter((node) => store.folderMap.get(node.id)?.parent_id === null)
    .sort((a, b) => a.name.localeCompare(b.name))

  const sortChildren = (node: TreeFolder) => {
    node.children.sort((a, b) => a.name.localeCompare(b.name))
    node.children.forEach(sortChildren)
  }
  roots.forEach(sortChildren)

  return roots
})

const currentFolderPathWithRoot = computed(() => {
  return [{ id: null as number | null, name: 'Root' }, ...store.folderPath.map((item) => ({ id: item.id, name: item.name }))]
})

const selectedCount = computed(() => store.selectedIds.length)

function formatBytes(bytes: number) {
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`
  return `${(bytes / (1024 * 1024 * 1024)).toFixed(1)} GB`
}

function compressionRatio(file: FileItem) {
  if (!file.original_size) return '0.0%'
  return `${((file.compressed_size / file.original_size) * 100).toFixed(1)}%`
}

function uploadLabelStatus(status: string) {
  if (status === 'queued') return 'Queued'
  if (status === 'uploading') return 'Uploading'
  if (status === 'processing') return 'Processing'
  if (status === 'done') return 'Done'
  return 'Failed'
}

function pickFiles() {
  uploadInput.value?.click()
}

async function onFilesChosen(event: Event) {
  const input = event.target as HTMLInputElement
  await store.uploadFiles(input.files)
  input.value = ''
}

function isExternalFileDrag(event: DragEvent) {
  const types = event.dataTransfer?.types
  if (!types) return false
  return Array.from(types).includes('Files')
}

function onUploadDragEnter(event: DragEvent) {
  if (!isExternalFileDrag(event)) return
  event.preventDefault()
  uploadDragDepth.value += 1
  uploadDragActive.value = true
}

function onUploadDragOver(event: DragEvent) {
  if (!isExternalFileDrag(event)) return
  event.preventDefault()
  if (event.dataTransfer) {
    event.dataTransfer.dropEffect = 'copy'
  }
  uploadDragActive.value = true
}

function onUploadDragLeave(event: DragEvent) {
  if (!isExternalFileDrag(event)) return
  uploadDragDepth.value = Math.max(0, uploadDragDepth.value - 1)
  if (uploadDragDepth.value === 0) {
    uploadDragActive.value = false
  }
}

async function onUploadDrop(event: DragEvent) {
  if (!isExternalFileDrag(event)) return
  event.preventDefault()
  event.stopPropagation()
  uploadDragDepth.value = 0
  uploadDragActive.value = false
  const files = event.dataTransfer?.files
  if (!files || files.length === 0) return
  await store.uploadFiles(files)
}

function onFileDragStart(fileId: number, event: DragEvent) {
  store.draggedFileId = fileId
  if (event.dataTransfer) {
    event.dataTransfer.effectAllowed = 'move'
    event.dataTransfer.setData('text/plain', String(fileId))
  }
}

function onFileDragEnd() {
  store.draggedFileId = null
  store.dropFolderId = null
}

function droppedIdsForCurrentDrag(): number[] {
  if (!store.draggedFileId) return []
  if (store.selectedIds.includes(store.draggedFileId) && store.selectedIds.length > 1) {
    return [...store.selectedIds]
  }
  return [store.draggedFileId]
}

function onFolderDragOver(folderId: number | null) {
  store.dropFolderId = folderId
}

function onFolderDragLeave(folderId: number | null) {
  if (store.dropFolderId === folderId) {
    store.dropFolderId = null
  }
}

async function onFolderDrop(folderId: number | null) {
  const ids = droppedIdsForCurrentDrag()
  store.draggedFileId = null
  store.dropFolderId = null
  if (ids.length === 0) return
  await store.moveSelectedTo(folderId, ids)
}

function onCardClick(file: FileItem, event: MouseEvent) {
  if (event.shiftKey || event.ctrlKey || event.metaKey) {
    store.toggleSelection(file.id)
  } else {
    store.setActiveFile(file.id)
  }
}

async function onMoveSelected() {
  const raw = moveTarget.value.trim()
  if (raw === '') {
    await store.moveSelectedTo(null)
    return
  }
  const folderId = Number.parseInt(raw, 10)
  if (!Number.isInteger(folderId) || folderId <= 0) {
    store.showToast('error', 'Select a valid destination folder ID or leave empty for root')
    return
  }
  await store.moveSelectedTo(folderId)
}

function openCreateFolderModal() {
  folderNameDraft.value = ''
  folderNameError.value = ''
  folderModalOpen.value = true
  void nextTick(() => {
    folderNameInput.value?.focus()
  })
}

function closeCreateFolderModal() {
  if (store.busyAction) return
  folderModalOpen.value = false
}

async function submitCreateFolder() {
  const name = folderNameDraft.value.trim()
  if (!name) {
    folderNameError.value = 'Folder name is required'
    void nextTick(() => {
      folderNameInput.value?.focus()
    })
    return
  }

  folderNameError.value = ''
  const ok = await store.createFolderWithName(name)
  if (ok) {
    folderModalOpen.value = false
    folderNameDraft.value = ''
  }
}

function openRenameFolderModal() {
  if (store.currentFolderId === null) return
  const folder = store.folderMap.get(store.currentFolderId)
  if (!folder) return
  renameFolderDraft.value = folder.name
  renameFolderError.value = ''
  renameModalOpen.value = true
  void nextTick(() => {
    renameFolderInput.value?.focus()
    renameFolderInput.value?.select()
  })
}

function closeRenameFolderModal() {
  if (store.busyAction) return
  renameModalOpen.value = false
}

async function submitRenameFolder() {
  const name = renameFolderDraft.value.trim()
  if (!name) {
    renameFolderError.value = 'Folder name is required'
    return
  }
  renameFolderError.value = ''
  const ok = await store.renameCurrentFolderWithName(name)
  if (ok) {
    renameModalOpen.value = false
  }
}

function openMoveFolderModal() {
  if (store.currentFolderId === null) return
  const folder = store.folderMap.get(store.currentFolderId)
  if (!folder) return
  moveFolderDraft.value = folder.parent_id === null ? '' : String(folder.parent_id)
  moveFolderError.value = ''
  moveModalOpen.value = true
  void nextTick(() => {
    moveFolderInput.value?.focus()
    moveFolderInput.value?.select()
  })
}

function closeMoveFolderModal() {
  if (store.busyAction) return
  moveModalOpen.value = false
}

async function submitMoveFolder() {
  const trimmed = moveFolderDraft.value.trim()
  let parentId: number | null = null

  if (trimmed !== '') {
    const parsed = Number.parseInt(trimmed, 10)
    if (!Number.isInteger(parsed) || parsed <= 0) {
      moveFolderError.value = 'Parent folder ID must be a positive number, or leave empty for root.'
      return
    }
    parentId = parsed
  }

  moveFolderError.value = ''
  const ok = await store.moveCurrentFolderTo(parentId)
  if (ok) {
    moveModalOpen.value = false
  }
}

function closeConfirmModal() {
  if (store.busyAction) return
  confirmModalOpen.value = false
  confirmModalAction = null
}

function openConfirmModal(
  title: string,
  message: string,
  actionLabel: string,
  onConfirm: () => Promise<boolean | void>,
  danger = false,
) {
  confirmModalTitle.value = title
  confirmModalMessage.value = message
  confirmModalActionLabel.value = actionLabel
  confirmModalDanger.value = danger
  confirmModalAction = onConfirm
  confirmModalOpen.value = true
}

async function submitConfirmModal() {
  if (!confirmModalAction) return
  const ok = await confirmModalAction()
  if (ok !== false) {
    confirmModalOpen.value = false
    confirmModalAction = null
  }
}

function openDeleteSelectedModal() {
  if (selectedCount.value === 0) return
  openConfirmModal(
    'Delete Files',
    `Delete ${selectedCount.value} selected file(s)? This action cannot be undone.`,
    'Delete',
    async () => {
      return await store.deleteSelectedIds([...store.selectedIds])
    },
    true,
  )
}

function openDeleteFolderModal() {
  if (store.currentFolderId === null) return
  const folder = store.folderMap.get(store.currentFolderId)
  if (!folder) return
  openConfirmModal(
    'Delete Folder',
    `Delete folder "${folder.name}"? It must be empty first.`,
    'Delete',
    async () => {
      return await store.deleteCurrentFolderConfirmed()
    },
    true,
  )
}

function onGlobalKeyDown(event: KeyboardEvent) {
  if ((folderModalOpen.value || renameModalOpen.value || moveModalOpen.value || confirmModalOpen.value) && event.key === 'Escape') {
    event.preventDefault()
    if (confirmModalOpen.value) {
      closeConfirmModal()
      return
    }
    if (moveModalOpen.value) {
      closeMoveFolderModal()
      return
    }
    if (renameModalOpen.value) {
      closeRenameFolderModal()
      return
    }
    closeCreateFolderModal()
    return
  }

  if (!store.lightboxOpen) return

  if (event.key === 'Escape') {
    event.preventDefault()
    store.closeLightbox()
  }
  if (event.key === 'ArrowRight') {
    event.preventDefault()
    store.lightboxNext()
  }
  if (event.key === 'ArrowLeft') {
    event.preventDefault()
    store.lightboxPrev()
  }
}

function onWindowDragOver(event: DragEvent) {
  if (!isExternalFileDrag(event)) return
  event.preventDefault()
}

function onWindowDrop(event: DragEvent) {
  if (!isExternalFileDrag(event)) return
  event.preventDefault()
}

onMounted(async () => {
  document.addEventListener('keydown', onGlobalKeyDown)
  window.addEventListener('dragover', onWindowDragOver)
  window.addEventListener('drop', onWindowDrop)
  try {
    await store.initialize()
  } catch (error) {
    const message = error instanceof Error ? error.message : 'Initialization failed'
    store.showToast('error', message)
  }
})

onBeforeUnmount(() => {
  document.removeEventListener('keydown', onGlobalKeyDown)
  window.removeEventListener('dragover', onWindowDragOver)
  window.removeEventListener('drop', onWindowDrop)
})
</script>

<template>
  <div class="layout">
    <header class="topbar">
      <div>
        <h1>K2K Drive</h1>
        <p>Signed in as <strong>{{ store.username }}</strong></p>
      </div>
      <div class="top-actions">
        <button class="ghost" @click="store.doClearLogs">Clear Logs</button>
        <a class="ghost-link" href="/admin/logs/view" target="_blank" rel="noopener">View Logs</a>
        <a class="ghost-link" href="/admin/logs/download">Download Logs</a>
        <button class="danger" @click="store.doLogout">Logout</button>
      </div>
    </header>

    <div class="workspace">
      <aside class="sidebar card">
        <div class="sidebar-head">
          <h2>Folders</h2>
          <button class="primary" @click="openCreateFolderModal" :disabled="store.busyAction">New</button>
        </div>

        <button
          class="root-node"
          :class="{ active: store.currentFolderId === null, drop: store.dropFolderId === null }"
          @click="store.openFolder(null)"
          @dragover.prevent="onFolderDragOver(null)"
          @dragleave="onFolderDragLeave(null)"
          @drop.prevent="onFolderDrop(null)"
        >
          Root
        </button>

        <ul class="tree-list">
          <FolderTreeNode
            v-for="node in folderTree"
            :key="node.id"
            :node="node"
            :active-folder-id="store.currentFolderId"
            :drop-folder-id="store.dropFolderId"
            @open="store.openFolder($event)"
            @drag-over="onFolderDragOver($event)"
            @drag-leave="onFolderDragLeave($event)"
            @drop-folder="onFolderDrop($event)"
          />
        </ul>
      </aside>

      <section
        class="main card"
        :class="{ 'upload-drop-active': uploadDragActive }"
        @dragenter="onUploadDragEnter"
        @dragover="onUploadDragOver"
        @dragleave="onUploadDragLeave"
        @drop="onUploadDrop"
      >
        <div class="toolbar">
          <div class="breadcrumbs">
            <button
              v-for="crumb in currentFolderPathWithRoot"
              :key="crumb.id === null ? 'root' : crumb.id"
              class="crumb"
              :class="{ active: crumb.id === store.currentFolderId }"
              @click="store.openFolder(crumb.id)"
            >
              {{ crumb.name }}
            </button>
          </div>

          <div class="toolbar-actions">
            <button class="ghost" @click="openRenameFolderModal" :disabled="store.currentFolderId === null">Rename</button>
            <button class="ghost" @click="openMoveFolderModal" :disabled="store.currentFolderId === null">Move</button>
            <button class="danger" @click="openDeleteFolderModal" :disabled="store.currentFolderId === null">Delete</button>
            <button class="primary" @click="pickFiles">Upload</button>
            <input ref="uploadInput" type="file" multiple hidden @change="onFilesChosen" />
          </div>
        </div>

        <div class="upload-drop-zone" :class="{ active: uploadDragActive }">
          Drag files from desktop and drop here to upload into <strong>{{ store.currentFolderLabel }}</strong>.
        </div>

        <div class="filter-row">
          <input v-model="store.search" class="search" type="text" placeholder="Search files" />
          <select v-model="store.typeFilter" class="select">
            <option value="all">All Types</option>
            <option value="image">Images</option>
            <option value="video">Videos</option>
            <option value="audio">Audio</option>
            <option value="doc">Docs</option>
            <option value="other">Other</option>
          </select>
          <select v-model="store.sortMode" class="select">
            <option value="newest">Newest</option>
            <option value="oldest">Oldest</option>
            <option value="name_asc">Name A-Z</option>
            <option value="name_desc">Name Z-A</option>
            <option value="size_desc">Size High-Low</option>
            <option value="size_asc">Size Low-High</option>
          </select>
          <button class="ghost" @click="store.listMode = store.listMode === 'grid' ? 'list' : 'grid'">
            {{ store.listMode === 'grid' ? 'List View' : 'Grid View' }}
          </button>
        </div>

        <div class="stats-row">
          <span class="chip">Visible: {{ store.filteredFiles.length }}</span>
          <span class="chip">Folder: {{ store.currentFolderLabel }}</span>
          <span class="chip">Original: {{ formatBytes(store.totalVisibleOriginal) }}</span>
          <span class="chip">Stored: {{ formatBytes(store.totalVisibleStored) }}</span>
          <span class="chip">All Files: {{ store.fileSummary.total_files }}</span>
        </div>

        <div v-if="selectedCount > 0" class="batch-bar">
          <span>{{ selectedCount }} selected</span>
          <button class="ghost" @click="store.toggleSelectAllVisible">{{ store.allFilteredSelected ? 'Unselect Visible' : 'Select Visible' }}</button>
          <input v-model="moveTarget" class="move-input" placeholder="Folder ID or empty=root" />
          <button class="ghost" @click="onMoveSelected" :disabled="store.busyAction">Move Selected</button>
          <button class="danger" @click="openDeleteSelectedModal" :disabled="store.busyAction">Delete Selected</button>
          <button class="ghost" @click="store.clearSelection">Clear</button>
        </div>

        <div v-if="store.loadingFiles" class="empty">Loading files...</div>

        <div v-else-if="store.filteredFiles.length === 0" class="empty">
          No files in this folder.
        </div>

        <div v-else :class="store.listMode === 'grid' ? 'file-grid' : 'file-list'">
          <article
            v-for="file in store.filteredFiles"
            :key="file.id"
            class="file-card"
            :class="{
              selected: store.selectedIds.includes(file.id),
              active: store.activeFileId === file.id,
            }"
            draggable="true"
            @dragstart="onFileDragStart(file.id, $event)"
            @dragend="onFileDragEnd"
            @click="onCardClick(file, $event)"
          >
            <div class="file-thumb" @dblclick.stop="store.openLightbox(file.id)">
              <img
                v-if="store.fileKind(file) === 'image'"
                :src="store.fileThumbUrl(file)"
                :alt="file.original_name"
                loading="lazy"
              />
              <video
                v-else-if="store.fileKind(file) === 'video'"
                :src="store.fileViewUrl(file)"
                muted
                preload="metadata"
              />
              <div v-else class="fallback">{{ store.fileKind(file).toUpperCase() }}</div>
            </div>

            <div class="file-body">
              <label class="checkline" @click.stop>
                <input
                  type="checkbox"
                  :checked="store.selectedIds.includes(file.id)"
                  @change="store.toggleSelection(file.id)"
                />
                <span>{{ file.original_name }}</span>
              </label>

              <p>Original: {{ formatBytes(file.original_size) }}</p>
              <p>Stored: {{ formatBytes(file.compressed_size) }}</p>
              <p>Ratio: {{ compressionRatio(file) }}</p>
              <p>Uploaded: {{ file.created_at }}</p>

              <div class="row-actions" @click.stop>
                <button class="ghost" @click="store.setActiveFile(file.id)">Preview</button>
                <button class="ghost" @click="store.openLightbox(file.id)">Viewer</button>
                <a class="ghost-link" :href="store.fileDownloadUrl(file)">Download</a>
              </div>
            </div>
          </article>
        </div>

        <div class="more-row" v-if="store.hasMore">
          <button class="ghost" :disabled="store.loadingMore" @click="store.loadMore">
            {{ store.loadingMore ? 'Loading...' : 'Load More' }}
          </button>
        </div>
      </section>

      <aside class="preview card">
        <template v-if="store.activeFile">
          <div class="preview-head">
            <h3>Preview</h3>
            <button class="ghost" @click="store.openLightbox(store.activeFile.id)">Open Viewer</button>
          </div>

          <div class="preview-media">
            <img
              v-if="store.fileKind(store.activeFile) === 'image'"
              :src="store.fileViewUrl(store.activeFile)"
              :alt="store.activeFile.original_name"
            />
            <video
              v-else-if="store.fileKind(store.activeFile) === 'video'"
              :src="store.fileViewUrl(store.activeFile)"
              controls
              preload="metadata"
            />
            <div v-else class="fallback large">{{ store.fileKind(store.activeFile).toUpperCase() }}</div>
          </div>

          <div class="meta">
            <p><strong>{{ store.activeFile.original_name }}</strong></p>
            <p>Original: {{ formatBytes(store.activeFile.original_size) }}</p>
            <p>Stored: {{ formatBytes(store.activeFile.compressed_size) }}</p>
            <p>Ratio: {{ compressionRatio(store.activeFile) }}</p>
            <p>Date: {{ store.activeFile.created_at }}</p>
            <a class="ghost-link" :href="store.fileDownloadUrl(store.activeFile)">Download</a>
          </div>
        </template>

        <div v-else class="empty small">Select a file to see preview and metadata.</div>
      </aside>
    </div>

    <div class="upload-stack" v-if="store.uploadTasks.length > 0">
      <div
        v-for="task in store.uploadTasks"
        :key="task.localId"
        class="upload-item"
        :class="task.status"
      >
        <div class="upload-name">{{ task.name }}</div>
        <div class="upload-status">{{ uploadLabelStatus(task.status) }}</div>
        <div v-if="task.error" class="upload-error">{{ task.error }}</div>
      </div>
    </div>

    <div v-if="store.toasts.length > 0" class="toast-stack">
      <div v-for="toast in store.toasts" :key="toast.id" class="toast-card" :class="toast.type">
        <div class="toast-head">
          <strong>{{ toast.title }}</strong>
          <button class="toast-close" @click="store.dismissToast(toast.id)">✕</button>
        </div>
        <p class="toast-message">{{ toast.message }}</p>
        <span class="toast-progress" :style="{ animationDuration: `${toast.timeoutMs}ms` }"></span>
      </div>
    </div>

    <div v-if="folderModalOpen" class="dialog-backdrop" @click.self="closeCreateFolderModal">
      <div class="dialog card">
        <h3>Create Folder</h3>
        <label class="field-label" for="new-folder-name">Folder name</label>
        <input
          id="new-folder-name"
          ref="folderNameInput"
          v-model="folderNameDraft"
          class="dialog-input"
          type="text"
          maxlength="120"
          placeholder="e.g. Photos 2026"
          @keydown.enter.prevent="submitCreateFolder"
        />
        <p v-if="folderNameError" class="field-error">{{ folderNameError }}</p>
        <div class="dialog-actions">
          <button class="ghost" @click="closeCreateFolderModal" :disabled="store.busyAction">Cancel</button>
          <button class="primary" @click="submitCreateFolder" :disabled="store.busyAction">Create</button>
        </div>
      </div>
    </div>

    <div v-if="renameModalOpen" class="dialog-backdrop" @click.self="closeRenameFolderModal">
      <div class="dialog card">
        <h3>Rename Folder</h3>
        <label class="field-label" for="rename-folder-name">New folder name</label>
        <input
          id="rename-folder-name"
          ref="renameFolderInput"
          v-model="renameFolderDraft"
          class="dialog-input"
          type="text"
          maxlength="120"
          @keydown.enter.prevent="submitRenameFolder"
        />
        <p v-if="renameFolderError" class="field-error">{{ renameFolderError }}</p>
        <div class="dialog-actions">
          <button class="ghost" @click="closeRenameFolderModal" :disabled="store.busyAction">Cancel</button>
          <button class="primary" @click="submitRenameFolder" :disabled="store.busyAction">Save</button>
        </div>
      </div>
    </div>

    <div v-if="moveModalOpen" class="dialog-backdrop" @click.self="closeMoveFolderModal">
      <div class="dialog card">
        <h3>Move Folder</h3>
        <label class="field-label" for="move-folder-parent">Parent folder ID</label>
        <input
          id="move-folder-parent"
          ref="moveFolderInput"
          v-model="moveFolderDraft"
          class="dialog-input"
          type="text"
          inputmode="numeric"
          placeholder="Leave empty for Root"
          @keydown.enter.prevent="submitMoveFolder"
        />
        <p class="field-hint">Tip: empty = Root.</p>
        <p v-if="moveFolderError" class="field-error">{{ moveFolderError }}</p>
        <div class="dialog-actions">
          <button class="ghost" @click="closeMoveFolderModal" :disabled="store.busyAction">Cancel</button>
          <button class="primary" @click="submitMoveFolder" :disabled="store.busyAction">Move</button>
        </div>
      </div>
    </div>

    <div v-if="confirmModalOpen" class="dialog-backdrop" @click.self="closeConfirmModal">
      <div class="dialog card">
        <h3>{{ confirmModalTitle }}</h3>
        <p class="confirm-message">{{ confirmModalMessage }}</p>
        <div class="dialog-actions">
          <button class="ghost" @click="closeConfirmModal" :disabled="store.busyAction">Cancel</button>
          <button
            :class="confirmModalDanger ? 'danger' : 'primary'"
            @click="submitConfirmModal"
            :disabled="store.busyAction"
          >
            {{ confirmModalActionLabel }}
          </button>
        </div>
      </div>
    </div>

    <div v-if="store.lightboxOpen && store.lightboxFile" class="lightbox" @click.self="store.closeLightbox">
      <button class="nav prev" @click="store.lightboxPrev">‹</button>
      <div class="lightbox-content">
        <img
          v-if="store.fileKind(store.lightboxFile) === 'image'"
          :src="store.fileViewUrl(store.lightboxFile)"
          :alt="store.lightboxFile.original_name"
        />
        <video
          v-else
          :src="store.fileViewUrl(store.lightboxFile)"
          controls
          autoplay
        />
        <div class="caption">{{ store.lightboxFile.original_name }}</div>
      </div>
      <button class="nav next" @click="store.lightboxNext">›</button>
      <button class="close" @click="store.closeLightbox">✕</button>
    </div>
  </div>
</template>
