import { defineConfig } from 'vite'
import vue from '@vitejs/plugin-vue'

const control = process.env.MQS_CONTROL || 'http://127.0.0.1:5701'

export default defineConfig({
  plugins: [vue()],
  server: {
    host: '0.0.0.0',
    port: 5173,
    proxy: {
      '/_mqs': { target: control, changeOrigin: false, ws: true },
      '/avatars': { target: control },
    },
  },
  build: { outDir: 'dist', emptyOutDir: true },
})
