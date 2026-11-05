import { describe, expect, it } from 'vitest';
import { build } from './helpers';

/** Asset hashes change with file contents, not with infrastructure. */
const stable = (value: unknown): unknown =>
  JSON.parse(JSON.stringify(value).replace(/[0-9a-f]{64}/g, 'ASSET_HASH'));

describe('template snapshots', () => {
  it.each(['dev', 'prod'] as const)('%s', (stage) => {
    expect(stable(build(stage).template.toJSON())).toMatchSnapshot();
  });
});
