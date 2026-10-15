import { defineConfig } from 'vitest/config';
import solidPlugin from "vite-plugin-solid";

export default defineConfig({
  plugins: [solidPlugin({ hot: false })],
  define: {
    __APP_VERSION__: JSON.stringify("test"),
    __BUILD_PROFILE__: JSON.stringify("test"),
  },
  test: {
    coverage: {
      provider: 'v8',
      include: ['src/**/*.{ts,tsx}'],
      exclude: [
        '**/*.{test,spec}.{ts,tsx}', '**/*.d.ts',
        'src/shared/testing/**', 'src/sidebar/testing/**',
        'src/sidebar/styles/css-test-helpers.ts',
        'src/sidebar/stores/project-archive-test-helpers.ts',
        'src/sidebar/components/ac-discovery-fixture.ts',
        'src/sidebar/components/loop-summary-fixture.ts',
        'src/sidebar/components/restart-toast-harness.tsx',
        'src/sidebar/components/settings/agentReorderDnd.testkit.ts',
      ],
      reporter: ['html', 'json', 'json-summary', 'text', 'text-summary'],
      reportsDirectory: process.env.COVERAGE_OUTPUT || 'artifacts/coverage/frontend',
      excludeAfterRemap: true,
      clean: false,
    },
    includeTaskLocation: true,
    environment: 'node',
    include: ['src/**/*.test.ts', 'src/**/*.test.tsx'],
  },
});
