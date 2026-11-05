import { Token, Validations, type App, type Stack } from 'aws-cdk-lib';
import { AwsSolutionsChecks } from 'cdk-nag';

/** Attach the cdk-nag AwsSolutions pack. Violations fail `App.synth()`. */
export function applyNag(app: App): void {
  Validations.of(app).addPlugins(new AwsSolutionsChecks(app));
}

/**
 * Render an ARN-bearing cdk-nag finding id the way cdk-nag prints it: the stack's partition and
 * account tokens become `<AWS::Partition>` / `<AWS::AccountId>`, or stay literal when the
 * `enablePartitionLiterals` flag is on.
 */
export function findingId(stack: Stack, id: string): string {
  const partition = Token.isUnresolved(stack.partition)
    ? '<AWS::Partition>'
    : stack.partition;
  const account = Token.isUnresolved(stack.account)
    ? '<AWS::AccountId>'
    : stack.account;
  return id
    .replaceAll(stack.partition, partition)
    .replaceAll(stack.account, account);
}
