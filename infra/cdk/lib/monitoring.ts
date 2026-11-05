import { Duration, Validations } from 'aws-cdk-lib';
import {
  Alarm,
  Metric,
  ComparisonOperator,
  TreatMissingData,
} from 'aws-cdk-lib/aws-cloudwatch';
import { SnsAction } from 'aws-cdk-lib/aws-cloudwatch-actions';
import type { IFunction } from 'aws-cdk-lib/aws-lambda';
import { Topic } from 'aws-cdk-lib/aws-sns';
import { EmailSubscription } from 'aws-cdk-lib/aws-sns-subscriptions';
import { Construct } from 'constructs';
import type { IHttpStage } from 'aws-cdk-lib/aws-apigatewayv2';
import type { StageConfig } from './config';

export interface MonitoringProps {
  readonly config: StageConfig;
  readonly functions: Readonly<Record<string, IFunction>>;
  readonly apiStage: IHttpStage;
  /** Name of the IoT topic rule to watch for failed actions. */
  readonly iotRuleName: string;
}

/** Error and throttle alarms into an SNS topic. It is subscribed only when the `alarmEmail` context is set. */
export class Monitoring extends Construct {
  readonly alarmTopic: Topic;

  constructor(scope: Construct, id: string, props: MonitoringProps) {
    super(scope, id);
    this.alarmTopic = new Topic(this, 'AlarmTopic', {
      topicName: `fotoko-${props.config.stage}-alarms`,
      enforceSSL: true,
    });
    if (props.config.alarmEmail) {
      this.alarmTopic.addSubscription(
        new EmailSubscription(props.config.alarmEmail),
      );
    }
    const action = new SnsAction(this.alarmTopic);
    const common = {
      threshold: 1,
      evaluationPeriods: 1,
      comparisonOperator: ComparisonOperator.GREATER_THAN_OR_EQUAL_TO_THRESHOLD,
      treatMissingData: TreatMissingData.NOT_BREACHING,
    };

    for (const [name, fn] of Object.entries(props.functions)) {
      new Alarm(this, `${name}Errors`, {
        ...common,
        alarmDescription: `${name} Lambda reported errors`,
        metric: fn.metricErrors({
          period: Duration.minutes(5),
          statistic: 'Sum',
        }),
      }).addAlarmAction(action);
    }
    for (const [name, fn] of Object.entries(props.functions)) {
      new Alarm(this, `${name}Throttles`, {
        ...common,
        alarmDescription: `${name} Lambda was throttled`,
        metric: fn.metricThrottles({
          period: Duration.minutes(5),
          statistic: 'Sum',
        }),
      }).addAlarmAction(action);
    }
    new Alarm(this, 'IotRuleFailures', {
      ...common,
      alarmDescription:
        'IoT topic rule failed to run an action (status updates are being dropped)',
      metric: new Metric({
        namespace: 'AWS/IoT',
        metricName: 'Failure',
        dimensionsMap: { RuleName: props.iotRuleName },
        period: Duration.minutes(5),
        statistic: 'Sum',
      }),
    }).addAlarmAction(action);
    new Alarm(this, 'Api5xx', {
      ...common,
      alarmDescription: 'HTTP API returned 5xx responses',
      metric: props.apiStage.metricServerError({
        period: Duration.minutes(5),
        statistic: 'Sum',
      }),
    }).addAlarmAction(action);

    Validations.of(this.alarmTopic).acknowledge({
      id: 'AwsSolutions-SNS2',
      reason:
        'CloudWatch alarms cannot publish to topics encrypted with the AWS-managed SNS key, and the messages carry no sensitive data.',
    });
  }
}
