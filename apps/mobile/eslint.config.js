const { defineConfig } = require('eslint/config');
const { fixupPluginRules } = require('@eslint/compat');
const expoConfig = require('eslint-config-expo/flat');

// eslint-plugin-react still uses context APIs removed in ESLint 10.
const compatibleExpoConfig = expoConfig.map((config) =>
  config.plugins?.react
    ? {
        ...config,
        plugins: {
          ...config.plugins,
          react: fixupPluginRules(config.plugins.react),
        },
      }
    : config,
);

module.exports = defineConfig([
  compatibleExpoConfig,
  { ignores: ['dist/*', 'coverage/*', '.expo/*', '.maestro/*'] },
  {
    files: ['*.js'],
    languageOptions: { globals: { __dirname: 'readonly' } },
  },
  {
    // Type-aware rules: a forgotten `await` on a payment or upload call must not pass lint.
    files: ['**/*.{ts,tsx}'],
    languageOptions: {
      parserOptions: { projectService: true, tsconfigRootDir: __dirname },
    },
    rules: {
      '@typescript-eslint/no-floating-promises': 'error',
      '@typescript-eslint/no-misused-promises': 'error',
    },
  },
]);
