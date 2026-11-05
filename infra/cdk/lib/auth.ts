import { Duration, Validations } from 'aws-cdk-lib';
import {
  CfnIdentityPool,
  CfnIdentityPoolRoleAttachment,
  UserPool,
} from 'aws-cdk-lib/aws-cognito';
import {
  Effect,
  FederatedPrincipal,
  PolicyStatement,
  Role,
} from 'aws-cdk-lib/aws-iam';
import { Construct } from 'constructs';
import type { StageConfig } from './config';

export interface AuthProps {
  readonly config: StageConfig;
  /** `execute-api:Invoke` ARNs of the IAM-authorised API routes. */
  readonly invokeArns: readonly string[];
  /** Acknowledgment ids for the wildcard ARNs in `invokeArns`. */
  readonly invokeArnFindings: readonly string[];
}

/** Guest identities (Cognito identity pool) and a placeholder operator user pool. */
export class Auth extends Construct {
  readonly identityPool: CfnIdentityPool;
  readonly guestRole: Role;
  readonly userPool: UserPool;

  constructor(scope: Construct, id: string, props: AuthProps) {
    super(scope, id);
    const { config } = props;

    this.identityPool = new CfnIdentityPool(this, 'IdentityPool', {
      identityPoolName: `fotoko_${config.stage}`,
      allowUnauthenticatedIdentities: true,
      allowClassicFlow: false,
    });

    this.guestRole = new Role(this, 'GuestRole', {
      description:
        'Cognito guest (unauthenticated) identities: invoke the order routes only.',
      assumedBy: new FederatedPrincipal(
        'cognito-identity.amazonaws.com',
        {
          StringEquals: {
            'cognito-identity.amazonaws.com:aud': this.identityPool.ref,
          },
          'ForAnyValue:StringLike': {
            'cognito-identity.amazonaws.com:amr': 'unauthenticated',
          },
        },
        'sts:AssumeRoleWithWebIdentity',
      ),
    });
    this.guestRole.addToPolicy(
      new PolicyStatement({
        effect: Effect.ALLOW,
        actions: ['execute-api:Invoke'],
        resources: [...props.invokeArns],
      }),
    );
    Validations.of(this.guestRole).acknowledge(
      ...props.invokeArnFindings.map((id) => ({
        id,
        reason:
          'Order ids are dynamic path segments; only the four order routes are invokable.',
      })),
    );
    Validations.of(this.identityPool).acknowledge({
      id: 'AwsSolutions-COG7',
      reason:
        'Guest sessions are the product: customers at a Stand upload and pay without signing in.',
    });
    new CfnIdentityPoolRoleAttachment(this, 'RoleAttachment', {
      identityPoolId: this.identityPool.ref,
      roles: { unauthenticated: this.guestRole.roleArn },
    });

    // Future use: operator sign-in. Nothing references it yet.
    this.userPool = new UserPool(this, 'OperatorUserPool', {
      userPoolName: `fotoko-${config.stage}-operators`,
      selfSignUpEnabled: false,
      signInAliases: { email: true },
      passwordPolicy: {
        minLength: 12,
        requireLowercase: true,
        requireUppercase: true,
        requireDigits: true,
        requireSymbols: true,
        tempPasswordValidity: Duration.days(3),
      },
      removalPolicy: config.removalPolicy,
    });
    this.userPool.addClient('OperatorAppClient', {
      generateSecret: false,
      authFlows: { userSrp: true },
    });
    Validations.of(this.userPool).acknowledge(
      {
        id: 'AwsSolutions-COG2',
        reason:
          'Placeholder pool for future operator sign-in; no users exist yet. Enable MFA before use.',
      },
      {
        id: 'AwsSolutions-COG8',
        reason:
          'Placeholder pool for future operator sign-in; the plus feature plan is chosen before use.',
      },
      {
        id: 'AwsSolutions-COG3',
        reason:
          'Placeholder pool for future operator sign-in; advanced security (paid) is enabled before use.',
      },
    );
  }
}
