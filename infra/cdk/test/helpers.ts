import { join } from 'node:path';
import { App } from 'aws-cdk-lib';
import { Template } from 'aws-cdk-lib/assertions';
import type { Stage } from '../lib/config';
import { FotokoStack, type FotokoStackProps } from '../lib/fotoko-stack';
import { applyNag } from '../lib/nag';

export const FIXTURE_LAMBDA_DIR = join(
  import.meta.dirname,
  'fixtures',
  'lambda',
);

export function build(
  stage: Stage,
  props: Partial<FotokoStackProps> = {},
  nag = false,
) {
  const app = new App();
  const stack = new FotokoStack(app, `Fotoko-${stage}`, {
    stage,
    lambdaDir: FIXTURE_LAMBDA_DIR,
    env: { account: '123456789012', region: 'ap-southeast-1' },
    ...props,
  });
  if (nag) applyNag(app);
  return { app, stack, template: Template.fromStack(stack) };
}

/** IAM statements of every policy in the template, with the role they attach to. */
export function policyStatements(
  template: Template,
): { roles: unknown; statement: Record<string, unknown> }[] {
  const policies = template.findResources('AWS::IAM::Policy');
  return Object.values(policies).flatMap((policy) => {
    const props = policy.Properties as {
      Roles: unknown;
      PolicyDocument: { Statement: Record<string, unknown>[] };
    };
    return props.PolicyDocument.Statement.map((statement) => ({
      roles: props.Roles,
      statement,
    }));
  });
}

export const asArray = <T>(value: T | T[]): T[] =>
  Array.isArray(value) ? value : [value];
