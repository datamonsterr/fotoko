import { Stack, Validations } from 'aws-cdk-lib';
import {
  Effect,
  PolicyDocument,
  PolicyStatement,
  Role,
  ServicePrincipal,
} from 'aws-cdk-lib/aws-iam';
import { CfnPolicy, CfnThingType, CfnTopicRule } from 'aws-cdk-lib/aws-iot';
import type { IFunction } from 'aws-cdk-lib/aws-lambda';
import { LogGroup } from 'aws-cdk-lib/aws-logs';
import {
  AwsCustomResource,
  AwsCustomResourcePolicy,
  PhysicalResourceId,
} from 'aws-cdk-lib/custom-resources';
import { Construct } from 'constructs';
import { devicePolicyName, thingTypeName, type StageConfig } from './config';

export const TOPIC_ROOT = 'fotoko/printers';
const THING_NAME = '${iot:Connection.Thing.ThingName}';

/** ARN pattern of every printer's jobs topic, the only topic the cloud publishes to. */
export function jobsTopicArnPattern(stack: Stack): string {
  return `arn:${stack.partition}:iot:${stack.region}:${stack.account}:topic/${TOPIC_ROOT}/*/jobs`;
}

export interface IotEndpointProps {
  readonly config: StageConfig;
}

/** The account's ATS data endpoint (`xxxx-ats.iot.<region>.amazonaws.com`), via `DescribeEndpoint`. */
export class IotEndpoint extends Construct {
  readonly address: string;

  constructor(scope: Construct, id: string, props: IotEndpointProps) {
    super(scope, id);
    // Own role and log group for the (stack-wide singleton) provider function: no managed policy,
    // explicit retention and removal policy.
    const logGroup = new LogGroup(this, 'ProviderLogs', {
      retention: props.config.logRetention,
      removalPolicy: props.config.removalPolicy,
    });
    const role = new Role(this, 'ProviderRole', {
      assumedBy: new ServicePrincipal('lambda.amazonaws.com'),
    });
    logGroup.grantWrite(role);
    const lookup = new AwsCustomResource(this, 'DescribeEndpoint', {
      installLatestAwsSdk: false,
      role,
      logGroup,
      onUpdate: {
        service: 'Iot',
        action: 'describeEndpoint',
        parameters: { endpointType: 'iot:Data-ATS' },
        physicalResourceId: PhysicalResourceId.of('iot-data-ats-endpoint'),
      },
      policy: AwsCustomResourcePolicy.fromStatements([
        // iot:DescribeEndpoint does not support resource-level permissions.
        new PolicyStatement({
          actions: ['iot:DescribeEndpoint'],
          resources: ['*'],
        }),
      ]),
    });
    this.address = lookup.getResponseField('endpointAddress');
    Validations.of(lookup).acknowledge({
      id: 'AwsSolutions-IAM5[Resource::*]',
      reason:
        'iot:DescribeEndpoint does not support resource-level permissions.',
    });
  }
}

export interface IotCoreProps {
  readonly config: StageConfig;
  /** Receives every `status` and `heartbeat` message. */
  readonly statusFunction: IFunction;
}

/** Thing type, device policy and the rule that feeds printer status to `fotoko-iot-status`. */
export class IotCore extends Construct {
  readonly thingType: CfnThingType;
  readonly devicePolicy: CfnPolicy;
  readonly topicRule: CfnTopicRule;
  readonly ruleName: string;

  constructor(scope: Construct, id: string, props: IotCoreProps) {
    super(scope, id);
    const stack = Stack.of(this);
    const arn = (resource: string): string =>
      `arn:${stack.partition}:iot:${stack.region}:${stack.account}:${resource}`;
    const own = `${TOPIC_ROOT}/${THING_NAME}`;

    this.thingType = new CfnThingType(this, 'PrinterThingType', {
      thingTypeName: thingTypeName(props.config.stage),
      thingTypeProperties: {
        thingTypeDescription: 'Fotoko Stand printer (Raspberry Pi)',
      },
    });

    // Policy variables are resolved by IoT, not CloudFormation: a Pi can only act as its own thing.
    this.devicePolicy = new CfnPolicy(this, 'DevicePolicy', {
      policyName: devicePolicyName(props.config.stage),
      policyDocument: new PolicyDocument({
        statements: [
          new PolicyStatement({
            effect: Effect.ALLOW,
            actions: ['iot:Connect'],
            resources: [arn(`client/${THING_NAME}`)],
          }),
          new PolicyStatement({
            effect: Effect.ALLOW,
            actions: ['iot:Publish'],
            resources: [
              arn(`topic/${own}/status`),
              arn(`topic/${own}/heartbeat`),
            ],
          }),
          new PolicyStatement({
            effect: Effect.ALLOW,
            actions: ['iot:Subscribe'],
            resources: [arn(`topicfilter/${own}/jobs`)],
          }),
          new PolicyStatement({
            effect: Effect.ALLOW,
            actions: ['iot:Receive'],
            resources: [arn(`topic/${own}/jobs`)],
          }),
        ],
      }).toJSON() as object,
    });

    // topic(1)='fotoko', topic(2)='printers', topic(3)=printerId, topic(4)=jobs|status|heartbeat.
    // The WHERE clause keeps the rule from firing on the cloud's own `jobs` publishes.
    this.ruleName = `fotoko_${props.config.stage}_printer_status`;
    // Failed rule actions (for example a throttled Lambda) are logged instead of dropped silently.
    const errorLogs = new LogGroup(this, 'RuleErrorLogs', {
      retention: props.config.logRetention,
      removalPolicy: props.config.removalPolicy,
    });
    const errorRole = new Role(this, 'RuleErrorRole', {
      assumedBy: new ServicePrincipal('iot.amazonaws.com'),
    });
    errorLogs.grantWrite(errorRole);
    this.topicRule = new CfnTopicRule(this, 'PrinterStatusRule', {
      ruleName: this.ruleName,
      topicRulePayload: {
        sql: `SELECT *, topic(3) AS printerId FROM '${TOPIC_ROOT}/+/+' WHERE topic(4) = 'status' OR topic(4) = 'heartbeat'`,
        awsIotSqlVersion: '2016-03-23',
        ruleDisabled: false,
        errorAction: {
          cloudwatchLogs: {
            logGroupName: errorLogs.logGroupName,
            roleArn: errorRole.roleArn,
          },
        },
        actions: [
          { lambda: { functionArn: props.statusFunction.functionArn } },
        ],
      },
    });
    props.statusFunction.addPermission('InvokeFromIotRule', {
      principal: new ServicePrincipal('iot.amazonaws.com'),
      sourceArn: this.topicRule.attrArn,
      sourceAccount: stack.account,
    });
  }
}
