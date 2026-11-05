import { Duration, Stack, Validations } from 'aws-cdk-lib';
import type { ITable } from 'aws-cdk-lib/aws-dynamodb';
import {
  Effect,
  PolicyStatement,
  Role,
  ServicePrincipal,
} from 'aws-cdk-lib/aws-iam';
import {
  Architecture,
  Code,
  Function as LambdaFunction,
  Runtime,
} from 'aws-cdk-lib/aws-lambda';
import { LogGroup } from 'aws-cdk-lib/aws-logs';
import type { CfnBucket, IBucket } from 'aws-cdk-lib/aws-s3';
import type { ISecret } from 'aws-cdk-lib/aws-secretsmanager';
import { Construct } from 'constructs';
import { lambdaAssetPath, type StageConfig } from './config';
import { GSI1, ORDERS_PREFIX } from './data';
import { jobsTopicArnPattern } from './iot';
import { iam5Resource } from './nag';

export interface FunctionsProps {
  readonly config: StageConfig;
  readonly lambdaDir: string;
  readonly table: ITable;
  readonly photosBucket: IBucket;
  readonly payosSecret: ISecret;
  readonly iotDataEndpoint: string;
  readonly publicAppUrl: string;
}

const HTTP_TABLE_ACTIONS = [
  'dynamodb:GetItem',
  'dynamodb:PutItem',
  'dynamodb:UpdateItem',
  'dynamodb:Query',
];
const STATUS_TABLE_ACTIONS = [
  'dynamodb:GetItem',
  'dynamodb:UpdateItem',
  'dynamodb:Query',
];

/** The two Rust Lambdas (arm64, `provided.al2023`) with their least-privilege roles. */
export class Functions extends Construct {
  readonly http: LambdaFunction;
  readonly iotStatus: LambdaFunction;

  constructor(scope: Construct, id: string, props: FunctionsProps) {
    super(scope, id);
    const { config, table, photosBucket } = props;
    const stack = Stack.of(this);

    // fotoko-iot-status only touches the table; the rest belongs to fotoko-http.
    const statusEnvironment = {
      FOTOKO_MODE: 'aws',
      TABLE_NAME: table.tableName,
    };
    const httpEnvironment = {
      ...statusEnvironment,
      BUCKET_NAME: photosBucket.bucketName,
      IOT_DATA_ENDPOINT: props.iotDataEndpoint,
      PAYOS_BASE_URL: config.payosBaseUrl,
      PAYOS_SECRET_ARN: props.payosSecret.secretArn,
      PUBLIC_APP_URL: props.publicAppUrl,
      DEFAULT_PRICE_VND: String(config.defaultPriceVnd),
    };

    const make = (
      name: string,
      logicalId: string,
      memorySize: number,
      timeout: Duration,
      environment: Record<string, string>,
    ) => {
      const logGroup = new LogGroup(this, `${logicalId}Logs`, {
        logGroupName: `/aws/lambda/fotoko-${config.stage}-${name}`,
        retention: config.logRetention,
        removalPolicy: config.removalPolicy,
      });
      // Inline log permissions on the exact log group instead of AWSLambdaBasicExecutionRole.
      const role = new Role(this, `${logicalId}Role`, {
        assumedBy: new ServicePrincipal('lambda.amazonaws.com'),
      });
      logGroup.grantWrite(role);
      const fn = new LambdaFunction(this, logicalId, {
        functionName: `fotoko-${config.stage}-${name}`,
        runtime: Runtime.PROVIDED_AL2023,
        architecture: Architecture.ARM_64,
        handler: 'bootstrap',
        code: Code.fromAsset(lambdaAssetPath(props.lambdaDir, name)),
        role,
        logGroup,
        memorySize,
        timeout,
        environment,
      });
      return { fn, role };
    };

    const http = make(
      'fotoko-http',
      'Http',
      256,
      Duration.seconds(15),
      httpEnvironment,
    );
    const status = make(
      'fotoko-iot-status',
      'IotStatus',
      128,
      Duration.seconds(10),
      statusEnvironment,
    );
    this.http = http.fn;
    this.iotStatus = status.fn;

    // DynamoDB: no Delete/Scan/Batch. Only fotoko-http reads gsi1 (webhook lookup by order code).
    http.role.addToPolicy(
      new PolicyStatement({
        effect: Effect.ALLOW,
        actions: HTTP_TABLE_ACTIONS,
        resources: [table.tableArn, `${table.tableArn}/index/${GSI1}`],
      }),
    );
    status.role.addToPolicy(
      new PolicyStatement({
        effect: Effect.ALLOW,
        actions: STATUS_TABLE_ACTIONS,
        resources: [table.tableArn],
      }),
    );

    // Presigned PUT/GET and HeadObject (authorised by s3:GetObject) on order photos only.
    http.role.addToPolicy(
      new PolicyStatement({
        effect: Effect.ALLOW,
        actions: ['s3:PutObject', 's3:GetObject'],
        resources: [photosBucket.arnForObjects(`${ORDERS_PREFIX}*`)],
      }),
    );
    // Without s3:ListBucket, HeadObject on a missing key answers 403 instead of 404.
    http.role.addToPolicy(
      new PolicyStatement({
        effect: Effect.ALLOW,
        actions: ['s3:ListBucket'],
        resources: [photosBucket.bucketArn],
        conditions: { StringLike: { 's3:prefix': `${ORDERS_PREFIX}*` } },
      }),
    );
    Validations.of(http.role).acknowledge(
      {
        id: iam5Resource(
          stack,
          `<${stack.getLogicalId(photosBucket.node.defaultChild as CfnBucket)}.Arn>/${ORDERS_PREFIX}*`,
        ),
        reason:
          'Photo object keys are dynamic (orders/{orderId}/{photoId}); access is limited to the orders/ prefix.',
      },
      {
        id: iam5Resource(stack, jobsTopicArnPattern(stack)),
        reason:
          'One jobs topic per printer; the printer id segment is dynamic and the topic suffix is fixed.',
      },
    );
    http.role.addToPolicy(
      new PolicyStatement({
        effect: Effect.ALLOW,
        actions: ['iot:Publish'],
        resources: [jobsTopicArnPattern(stack)],
      }),
    );
    http.role.addToPolicy(
      new PolicyStatement({
        effect: Effect.ALLOW,
        actions: ['secretsmanager:GetSecretValue'],
        resources: [props.payosSecret.secretArn],
      }),
    );
  }
}
