import { App, Stack } from 'aws-cdk-lib';
import { Bucket } from 'aws-cdk-lib/aws-s3';
import { describe, expect, it } from 'vitest';
import { applyNag } from '../lib/nag';
import { build } from './helpers';

describe('cdk-nag AwsSolutions', () => {
  it.each(['dev', 'prod'] as const)(
    'has no unacknowledged findings in %s',
    (stage) => {
      const { app } = build(stage, {}, true);
      expect(() => app.synth()).not.toThrow();
    },
  );

  it('is wired up: a plain bucket fails the same checks', () => {
    const app = new App();
    new Bucket(new Stack(app, 'Bare'), 'Plain');
    applyNag(app);
    expect(() => app.synth()).toThrow();
  });
});
