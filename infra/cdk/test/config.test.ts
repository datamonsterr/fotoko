import { describe, expect, it } from 'vitest';
import {
  lambdaDir,
  parseOrigins,
  parseStage,
  stageConfig,
} from '../lib/config';

describe('parseOrigins', () => {
  it.each([
    'https://app.example.com',
    'https://app.example.com:8443',
    'http://localhost:8081',
    'http://127.0.0.1:3000',
  ])('accepts %s', (origin) => {
    expect(parseOrigins([origin])).toEqual([origin]);
  });

  it.each([
    '*',
    'https://*.example.com',
    'http://app.example.com',
    'https://app.example.com/',
    'https://app.example.com/path',
    'https://app.example.com?x=1',
    'app.example.com',
    '',
  ])('rejects %j', (origin) => {
    expect(() => parseOrigins([origin])).toThrow(/Invalid origin/);
  });
});

describe('stageConfig', () => {
  it('rejects bad extra origins', () => {
    expect(() => stageConfig('prod', { extraOrigins: ['*'] })).toThrow(
      /Invalid origin/,
    );
  });

  it('keeps the payOS URL and default price in typed config', () => {
    const config = stageConfig('dev');
    expect(config.payosBaseUrl).toBe('https://api-merchant.payos.vn');
    expect(config.defaultPriceVnd).toBe(10000);
  });

  it('orders budgets: webhook own, order routes below the stage default', () => {
    for (const stage of ['dev', 'prod'] as const) {
      const c = stageConfig(stage);
      expect(c.orderThrottle.rateLimit).toBeLessThan(c.throttle.rateLimit);
    }
  });
});

describe('parseStage', () => {
  it('defaults to dev and rejects unknown stages', () => {
    expect(parseStage(undefined)).toBe('dev');
    expect(() => parseStage('staging')).toThrow(/Invalid stage/);
  });
});

describe('lambdaDir', () => {
  it('resolves relative paths from the repo root and keeps absolute ones', () => {
    expect(lambdaDir({ FOTOKO_LAMBDA_DIR: '/abs/lambda' })).toBe('/abs/lambda');
    expect(lambdaDir({ FOTOKO_LAMBDA_DIR: 'infra/cdk/x' })).toMatch(
      /\/infra\/cdk\/x$/,
    );
    expect(lambdaDir({})).toMatch(/\/target\/lambda$/);
  });
});
