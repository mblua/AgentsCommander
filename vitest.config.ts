import { defineConfig } from 'vitest/config';
import solidPlugin from "vite-plugin-solid";

export default defineConfig({
  plugins: [solidPlugin({ hot: false })],
  define: {
    __APP_VERSION__: JSON.stringify("test"),
    __BUILD_PROFILE__: JSON.stringify("test"),
  },
  test: {
    includeTaskLocation: true,
    environment: 'node',
    include: ['src/**/*.test.ts', 'src/**/*.test.tsx'],
  },
});
