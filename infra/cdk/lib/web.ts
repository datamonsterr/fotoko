import { existsSync } from 'node:fs';
import {
  DefaultStackSynthesizer,
  Duration,
  Stack,
  Validations,
} from 'aws-cdk-lib';
import {
  AllowedMethods,
  Distribution,
  HttpVersion,
  PriceClass,
  ResponseHeadersPolicy,
  ViewerProtocolPolicy,
} from 'aws-cdk-lib/aws-cloudfront';
import { S3BucketOrigin } from 'aws-cdk-lib/aws-cloudfront-origins';
import {
  BlockPublicAccess,
  Bucket,
  BucketEncryption,
  type CfnBucket,
} from 'aws-cdk-lib/aws-s3';
import { BucketDeployment, Source } from 'aws-cdk-lib/aws-s3-deployment';
import { Role, ServicePrincipal } from 'aws-cdk-lib/aws-iam';
import { LogGroup } from 'aws-cdk-lib/aws-logs';
import { Construct } from 'constructs';
import { WEB_DIST_DIR, type StageConfig } from './config';
import { iam5Resource } from './nag';

export interface WebProps {
  readonly config: StageConfig;
  /** Upload `apps/mobile/dist` with the stack. Needs the Expo web export to exist. */
  readonly deployDist?: boolean;
  readonly distDir?: string;
}

/** S3 + CloudFront (OAC) hosting for the Expo web export, with SPA fallback to `index.html`. */
export class Web extends Construct {
  readonly bucket: Bucket;
  readonly distribution: Distribution;
  /** `https://<distribution domain>`; also the browser origin for CORS and `PUBLIC_APP_URL`. */
  readonly url: string;

  constructor(scope: Construct, id: string, props: WebProps) {
    super(scope, id);
    const { config } = props;
    const stack = Stack.of(this);

    this.bucket = new Bucket(this, 'Bucket', {
      blockPublicAccess: BlockPublicAccess.BLOCK_ALL,
      encryption: BucketEncryption.S3_MANAGED,
      enforceSSL: true,
      removalPolicy: config.removalPolicy,
      autoDeleteObjects: !config.isProd,
    });

    const spaFallback = {
      responseHttpStatus: 200,
      responsePagePath: '/index.html',
      ttl: Duration.seconds(0),
    };
    this.distribution = new Distribution(this, 'Distribution', {
      comment: `fotoko-${config.stage} web`,
      defaultRootObject: 'index.html',
      defaultBehavior: {
        origin: S3BucketOrigin.withOriginAccessControl(this.bucket),
        viewerProtocolPolicy: ViewerProtocolPolicy.REDIRECT_TO_HTTPS,
        allowedMethods: AllowedMethods.ALLOW_GET_HEAD,
        responseHeadersPolicy: ResponseHeadersPolicy.SECURITY_HEADERS,
        compress: true,
      },
      // SPA fallback: unknown paths return index.html so client-side routes (/orders/{id}) load.
      // Trade-off: a real missing asset or an S3 error also shows index.html with 200, not a 404.
      // S3 answers 403 for missing keys when the caller lacks ListBucket.
      errorResponses: [
        { ...spaFallback, httpStatus: 403 },
        { ...spaFallback, httpStatus: 404 },
      ],
      httpVersion: HttpVersion.HTTP2_AND_3,
      priceClass: PriceClass.PRICE_CLASS_200,
    });
    this.url = `https://${this.distribution.distributionDomainName}`;

    const distDir = props.distDir ?? WEB_DIST_DIR;
    if (props.deployDist === true) {
      if (!existsSync(distDir)) {
        throw new Error(
          `deployWeb is set but ${distDir} does not exist; run the Expo web export first.`,
        );
      }
      // Own role and log group for the deployment handler: no managed policy, explicit retention.
      const logGroup = new LogGroup(this, 'DeploymentLogs', {
        retention: config.logRetention,
        removalPolicy: config.removalPolicy,
      });
      const role = new Role(this, 'DeploymentRole', {
        assumedBy: new ServicePrincipal('lambda.amazonaws.com'),
      });
      logGroup.grantWrite(role);
      new BucketDeployment(this, 'Deployment', {
        sources: [Source.asset(distDir)],
        destinationBucket: this.bucket,
        distribution: this.distribution,
        distributionPaths: ['/*'],
        role,
        logGroup,
      });
      Validations.of(role).acknowledge(
        ...[
          'AwsSolutions-IAM5[Action::s3:GetObject*]',
          'AwsSolutions-IAM5[Action::s3:GetBucket*]',
          'AwsSolutions-IAM5[Action::s3:List*]',
          'AwsSolutions-IAM5[Action::s3:DeleteObject*]',
          'AwsSolutions-IAM5[Action::s3:Abort*]',
          `AwsSolutions-IAM5[Resource::<${stack.getLogicalId(this.bucket.node.defaultChild as CfnBucket)}.Arn>/*]`,
          iam5Resource(
            stack,
            `arn:${stack.partition}:s3:::cdk-${DefaultStackSynthesizer.DEFAULT_QUALIFIER}-assets-${stack.account}-${stack.region}/*`,
          ),
          'AwsSolutions-IAM5[Resource::*]',
        ].map((id) => ({
          id,
          reason:
            'BucketDeployment grants (CDK-generated) copy the export from the CDK asset bucket, prune the web bucket and invalidate CloudFront (invalidations have no resource-level permissions).',
        })),
      );
      // The deployment handler is a stack-wide singleton owned by CDK; its runtime follows the CDK release.
      for (const child of stack.node.findAll()) {
        if (child.node.id.startsWith('Custom::CDKBucketDeployment')) {
          Validations.of(child).acknowledge({
            id: 'AwsSolutions-L1',
            reason:
              'Runtime of the CDK-managed BucketDeployment handler is set by aws-cdk-lib.',
          });
        }
      }
    }

    Validations.of(this.bucket).acknowledge({
      id: 'AwsSolutions-S1',
      reason:
        'Static web assets only; no server access log bucket is warranted.',
    });
    Validations.of(this.distribution).acknowledge(
      {
        id: 'AwsSolutions-CFR1',
        reason:
          'The kiosk serves customers at physical Stands in Vietnam; geo restriction is not wanted.',
      },
      {
        id: 'AwsSolutions-CFR2',
        reason:
          'Public static assets of the Expo app; WAF cost is not justified.',
      },
      {
        id: 'AwsSolutions-CFR3',
        reason:
          'Access logging for a static, non-sensitive site is deferred; no log bucket to pay for.',
      },
      {
        id: 'AwsSolutions-CFR4',
        reason:
          'The default CloudFront certificate (*.cloudfront.net) pins the viewer TLS policy; a custom domain and certificate are future work.',
      },
    );
  }
}
