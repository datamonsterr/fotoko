import { Duration, Validations } from 'aws-cdk-lib';
import {
  Alarm,
  ComparisonOperator,
  TreatMissingData,
} from 'aws-cdk-lib/aws-cloudwatch';
import { SnsAction } from 'aws-cdk-lib/aws-cloudwatch-actions';
import type { IFunction } from 'aws-cdk-lib/aws-lambda';
import { Topic } from 'aws-cdk-lib/aws-sns';
import { Construct } from 'constructs';
import type { IHttpStage } from 'aws-cdk-lib/aws-apigatewayv2';
import type { StageConfig } from './config';

export interface MonitoringProps {
  readonly config: StageConfig;
  readonly functions: Readonly<Record<string, IFunction>>;
  readonly apiStage: IHttpStage;
}

/** Error alarms that notify an SNS topic. The topic has no subscription; operators add theirs. */
export class Monitoring extends Construct {
  readonly alarmTopic: Topic;

  constructor(scope: Construct, id: string, props: MonitoringProps) {
    super(scope, id);
    this.alarmTopic = new Topic(this, 'AlarmTopic', {
      topicName: `fotoko-${props.config.stage}-alarms`,
      enforceSSL: true,
    });
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
