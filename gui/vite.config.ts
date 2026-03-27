import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'

// https://vite.dev/config/
export default defineConfig({
  plugins: [react(), tailwindcss()],
  server: {
    proxy: {
      '/health': 'http://localhost:7420',
      '/stats': 'http://localhost:7420',
      '/config': 'http://localhost:7420',
      '/cache': 'http://localhost:7420',
      '/api': 'http://localhost:7420',
    },
  },
})
