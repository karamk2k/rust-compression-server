<script setup lang="ts">
interface TreeFolder {
  id: number
  name: string
  file_count: number
  children: TreeFolder[]
}

const props = defineProps<{
  node: TreeFolder
  activeFolderId: number | null
  dropFolderId: number | null
}>()

const emit = defineEmits<{
  open: [id: number]
  dragOver: [id: number]
  dragLeave: [id: number]
  dropFolder: [id: number]
}>()
</script>

<template>
  <li class="tree-item">
    <button
      class="tree-button"
      :class="{
        active: activeFolderId === node.id,
        drop: dropFolderId === node.id,
      }"
      @click="emit('open', node.id)"
      @dragover.prevent="emit('dragOver', node.id)"
      @dragleave="emit('dragLeave', node.id)"
      @drop.prevent="emit('dropFolder', node.id)"
      type="button"
    >
      <span class="name">{{ node.name }}</span>
      <span class="count">{{ node.file_count }}</span>
    </button>

    <ul v-if="node.children.length > 0" class="tree-list">
      <FolderTreeNode
        v-for="child in node.children"
        :key="child.id"
        :node="child"
        :active-folder-id="activeFolderId"
        :drop-folder-id="dropFolderId"
        @open="emit('open', $event)"
        @drag-over="emit('dragOver', $event)"
        @drag-leave="emit('dragLeave', $event)"
        @drop-folder="emit('dropFolder', $event)"
      />
    </ul>
  </li>
</template>
