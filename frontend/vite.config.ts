import { defineConfig } from 'vite'
import vue from '@vitejs/plugin-vue'
import { resolve } from 'node:path'

export default defineConfig({
  base: '/assets/app/',
  plugins: [vue()],
  build: {
    outDir: '../web/assets/app',
    emptyOutDir: true,
    cssCodeSplit: false,
    rollupOptions: {
      input: {
        app: resolve(__dirname, 'index.html'),
        live: resolve(__dirname, 'src/live.ts'),
        realtime: resolve(__dirname, 'src/realtime.ts'),
      },
      output: {
        minifyInternalExports: false,
        entryFileNames: '[name].js',
        chunkFileNames: 'chunk-[name].js',
        assetFileNames: (assetInfo) => {
          if (assetInfo.name === 'style.css') return 'app.css'
          return '[name][extname]'
        },
      },
    },
  },
})
