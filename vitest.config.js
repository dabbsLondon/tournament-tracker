import { defineConfig } from 'vitest/config';

export default defineConfig({
  test: {
    coverage: {
      provider: 'v8',
      include: ['static/js/lib/**/*.js'],
      thresholds: { lines: 80, functions: 80, branches: 80 },
    },
  },
});
