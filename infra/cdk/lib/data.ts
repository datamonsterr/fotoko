import { Duration, Validations } from 'aws-cdk-lib';
import {
  AttributeType,
  BillingMode,
  ProjectionType,
  Table,
  TableEncryption,
} from 'aws-cdk-lib/aws-dynamodb';
import {
  BlockPublicAccess,
  Bucket,
  BucketEncryption,
  HttpMethods,
  ObjectOwnership,
} from 'aws-cdk-lib/aws-s3';
import { Secret } from 'aws-cdk-lib/aws-secretsmanager';
import { Construct } from 'constructs';
import type { StageConfig } from './config';

export const GSI1 = 'gsi1';
/** S3 key prefix of order photos (`orders/{orderId}/{photoId}.{jpg|png}`). */
export const ORDERS_PREFIX = 'orders/';

export interface DataProps {
  readonly config: StageConfig;
  /** Browser origins allowed to PUT/GET photos with presigned URLs. */
  readonly appOrigins: readonly string[];
}

/** Single DynamoDB table, photos bucket and the payOS secret. */
export class Data extends Construct {
  readonly table: Table;
  readonly photosBucket: Bucket;
  readonly payosSecret: Secret;

  constructor(scope: Construct, id: string, props: DataProps) {
    super(scope, id);
    const { config } = props;

    this.table = new Table(this, 'Table', {
      tableName: `fotoko-${config.stage}`,
      partitionKey: { name: 'pk', type: AttributeType.STRING },
      sortKey: { name: 'sk', type: AttributeType.STRING },
      billingMode: BillingMode.PAY_PER_REQUEST,
      timeToLiveAttribute: 'ttl',
      encryption: TableEncryption.AWS_MANAGED,
      pointInTimeRecoverySpecification: {
        pointInTimeRecoveryEnabled: config.pointInTimeRecovery,
      },
      deletionProtection: config.isProd,
      removalPolicy: config.removalPolicy,
    });
    if (!config.pointInTimeRecovery) {
      Validations.of(this.table).acknowledge({
        id: 'AwsSolutions-DDB3',
        reason: `Point-in-time recovery is enabled for prod only; ${config.stage} data is disposable.`,
      });
    }
    this.table.addGlobalSecondaryIndex({
      indexName: GSI1,
      partitionKey: { name: 'gsi1pk', type: AttributeType.STRING },
      projectionType: ProjectionType.ALL,
    });

    this.photosBucket = new Bucket(this, 'PhotosBucket', {
      blockPublicAccess: BlockPublicAccess.BLOCK_ALL,
      encryption: BucketEncryption.S3_MANAGED,
      enforceSSL: true,
      objectOwnership: ObjectOwnership.BUCKET_OWNER_ENFORCED,
      removalPolicy: config.removalPolicy,
      autoDeleteObjects: !config.isProd,
      cors: [
        {
          allowedMethods: [HttpMethods.PUT, HttpMethods.GET],
          allowedOrigins: [...props.appOrigins],
          allowedHeaders: ['*'],
          exposedHeaders: ['ETag'],
          maxAge: 3000,
        },
      ],
      lifecycleRules: [
        {
          id: 'expire-order-photos',
          prefix: ORDERS_PREFIX,
          expiration: Duration.days(7),
          abortIncompleteMultipartUploadAfter: Duration.days(1),
        },
      ],
    });
    Validations.of(this.photosBucket).acknowledge({
      id: 'AwsSolutions-S1',
      reason:
        'Short-lived customer photos (7 days); server access logs would outlive the data they describe.',
    });

    // Placeholder structure only: operators overwrite clientId and apiKey, checksumKey is generated.
    this.payosSecret = new Secret(this, 'PayosSecret', {
      description:
        'payOS credentials {clientId,apiKey,checksumKey}. Replace the placeholders after deploy.',
      removalPolicy: config.removalPolicy,
      generateSecretString: {
        secretStringTemplate: JSON.stringify({
          clientId: 'REPLACE_ME',
          apiKey: 'REPLACE_ME',
        }),
        generateStringKey: 'checksumKey',
        excludePunctuation: true,
        passwordLength: 40,
      },
    });
    Validations.of(this.payosSecret).acknowledge({
      id: 'AwsSolutions-SMG4',
      reason:
        'payOS keys are issued by the provider and rotated manually in the payOS dashboard.',
    });
  }
}
