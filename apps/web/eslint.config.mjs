import { defineConfig, globalIgnores } from 'eslint/config';
import nextVitals from 'eslint-config-next/core-web-vitals';
import nextTs from 'eslint-config-next/typescript';

export default defineConfig([
  ...nextVitals,
  ...nextTs,
  {
    rules: {
      '@typescript-eslint/no-unused-vars': ['error', { argsIgnorePattern: '^_' }],
      // React Compiler rules added by eslint-config-next 16 / react-hooks 7; the existing (working, tested)
      // components predate them. Warn like legacy warptalk-web does, fix incrementally.
      'react-hooks/set-state-in-effect': 'warn',
      'react-hooks/refs': 'warn',
    },
  },
  {
    // Playwright fixtures use a `use` callback and empty destructuring by design.
    files: ['e2e/**/*.ts'],
    rules: { 'react-hooks/rules-of-hooks': 'off', 'no-empty-pattern': 'off' },
  },
  globalIgnores(['.next/**', 'out/**', 'next-env.d.ts', 'node_modules/**', 'test-results/**', 'playwright-report/**', 'e2e/__screenshots__/**', '.next*/**']),
]);
