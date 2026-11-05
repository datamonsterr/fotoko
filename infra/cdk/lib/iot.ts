import { Stack, Validations } from 'aws-cdk-lib';
import { Effect, PolicyDocument, PolicyStatement } from 'aws-cdk-lib/aws-iam';
import { CfnPolicy, CfnThingType, CfnTopicRule } from 'aws-cdk-lib/aws-iot';
import type { IFunction } from 'aws-cdk-lib/aws-lambda';
import { ServicePrincipal } from 'aws-cdk-lib/aws-iam';
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

/** The account's ATS data endpoint (`xxxx-ats.iot.<region>.amazonaws.com`), via `DescribeEndpoint`. */
export class IotEndpoint extends Construct {
  readonly address: string;

  constructor(scope: Construct, id: string) {
    super(scope, id);
    const lookup = new AwsCustomResource(this, 'DescribeEndpoint', {
      installLatestAwsSdk: false,
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
    // The singleton provider function shared by every AwsCustomResource in the stack.
    const provider = Stack.of(this).node.findChild(
      `AWS${AwsCustomResource.PROVIDER_FUNCTION_UUID.replace(/-/g, '')}`,
    );
    Validations.of(provider).acknowledge({
      id: 'AwsSolutions-IAM4[Policy::arn:<AWS::Partition>:iam::aws:policy/service-role/AWSLambdaBasicExecutionRole]',
      reason:
        'CDK-managed provider role of the custom resource that looks up the IoT endpoint.',
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
    this.topicRule = new CfnTopicRule(this, 'PrinterStatusRule', {
      ruleName: `fotoko_${props.config.stage}_printer_status`,
      topicRulePayload: {
        sql: `SELECT *, topic(3) AS printerId FROM '${TOPIC_ROOT}/+/+' WHERE topic(4) = 'status' OR topic(4) = 'heartbeat'`,
        awsIotSqlVersion: '2016-03-23',
        ruleDisabled: false,
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
