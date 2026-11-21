import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import tailwindcss from '@tailwindcss/vite';
import { fileURLToPath } from 'node:url';

const apiTarget = process.env.VITE_API_PROXY ?? 'http://127.0.0.1:18080';

export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: { alias: { '@': fileURLToPath(new URL('./src', import.meta.url)) } },
  server: {
    port: Number(process.env.WEB_PORT ?? 5173),
    host: '127.0.0.1',
    proxy: {
      '/api': { target: apiTarget, changeOrigin: true },
    },
  },
  build: { sourcemap: true },
});
