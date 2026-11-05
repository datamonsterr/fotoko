module.exports = {
  preset: 'jest-expo',
  setupFiles: ['<rootDir>/src/test/setup.ts'],
  moduleNameMapper: { '^@/(.*)$': '<rootDir>/src/$1' },
  testPathIgnorePatterns: ['/node_modules/', '/dist/'],
  collectCoverageFrom: [
    'src/**/*.{ts,tsx}',
    '!src/test/**',
    '!src/**/*.test.*',
  ],
};
