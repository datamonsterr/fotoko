import { Token, Validations, type App, type Stack } from 'aws-cdk-lib';
import { AwsSolutionsChecks } from 'cdk-nag';

/** Attach the cdk-nag AwsSolutions pack. Violations fail `App.synth()`. */
export function applyNag(app: App): void {
  Validations.of(app).addPlugins(new AwsSolutionsChecks(app));
}

/**
 * cdk-nag id of an `AwsSolutions-IAM5` wildcard finding on `resource`, an ARN built from this
 * stack's `partition` and `account` (or a `<LogicalId.Arn>/...` GetAtt rendering). cdk-nag prints
 * unresolved partition and account tokens as `<AWS::Partition>` and `<AWS::AccountId>`.
 */
export function iam5Resource(stack: Stack, resource: string): string {
  const partition = Token.isUnresolved(stack.partition)
    ? '<AWS::Partition>'
    : stack.partition;
  const account = Token.isUnresolved(stack.account)
    ? '<AWS::AccountId>'
    : stack.account;
  const rendered = resource
    .replace(`arn:${stack.partition}:`, `arn:${partition}:`)
    .replaceAll(stack.account, account);
  return `AwsSolutions-IAM5[Resource::${rendered}]`;
}
