import { describe, expect, it } from 'vitest';
import { asArray, build, policyStatements } from './helpers';

const { template } = build('prod');
const all = policyStatements(template);
const json = (value: unknown): string => JSON.stringify(value);
const actionsOf = (statement: Record<string, unknown>): string[] =>
  asArray(statement.Action as string | string[]);

/** Flatten an `Fn::Join` of literals and `AWS::Partition`/`AWS::AccountId` refs into a string. */
const render = (value: unknown): string => {
  if (typeof value === 'string') return value;
  const node = value as { 'Fn::Join'?: [string, unknown[]]; Ref?: string };
  if (node.Ref === 'AWS::Partition') return 'aws';
  if (node.Ref === 'AWS::AccountId') return '123456789012';
  if (node['Fn::Join'])
    return node['Fn::Join'][1].map(render).join(node['Fn::Join'][0]);
  throw new Error(`Cannot render ${JSON.stringify(value)}`);
};

const one = <T>(items: T[]): T => {
  const [item] = items;
  if (items.length !== 1 || item === undefined)
    throw new Error(`expected exactly one, got ${items.length}`);
  return item;
};

const byAction = (action: string) =>
  all.filter(({ statement }) => actionsOf(statement).includes(action));

describe('no broad IAM', () => {
  it('never grants a wildcard action', () => {
    for (const { statement } of all) {
      for (const action of actionsOf(statement))
        expect(action).not.toMatch(/\*/);
    }
  });

  it('never grants Resource "*" except iot:DescribeEndpoint for the endpoint lookup', () => {
    for (const { statement } of all) {
      const wildcardResource = asArray(
        statement.Resource as string | string[],
      ).includes('*');
      if (wildcardResource)
        expect(actionsOf(statement)).toEqual(['iot:DescribeEndpoint']);
    }
  });

  it('uses no managed policies on any role', () => {
    const roles = template.findResources('AWS::IAM::Role');
    const managed = Object.entries(roles)
      .filter(
        ([, r]) =>
          (r.Properties as { ManagedPolicyArns?: unknown })
            .ManagedPolicyArns !== undefined,
      )
      .map(([id]) => id);
    expect(managed).toEqual([]);
  });
});

describe('Lambda roles', () => {
  it('limits fotoko-http to four item actions on the table and gsi1', () => {
    const statements = byAction('dynamodb:PutItem');
    const { statement, roles } = one(statements);
    expect(actionsOf(statement).sort()).toEqual([
      'dynamodb:GetItem',
      'dynamodb:PutItem',
      'dynamodb:Query',
      'dynamodb:UpdateItem',
    ]);
    const resources = asArray(statement.Resource as unknown[]);
    expect(resources).toHaveLength(2);
    expect(json(resources[0])).toContain('DataTable');
    expect(json(resources[1])).toContain('/index/gsi1');
    expect(json(roles)).toContain('HttpRole');
  });

  it('limits fotoko-iot-status to get, update and query on the table only', () => {
    const { statement, roles } = one(
      byAction('dynamodb:GetItem').filter(({ roles: r }) =>
        json(r).includes('IotStatusRole'),
      ),
    );
    expect(actionsOf(statement).sort()).toEqual([
      'dynamodb:GetItem',
      'dynamodb:Query',
      'dynamodb:UpdateItem',
    ]);
    const resources = asArray(statement.Resource as unknown[]);
    expect(resources).toHaveLength(1);
    expect(json(resources[0])).toContain('DataTable');
    expect(json(resources[0])).not.toContain('/index/');
    expect(json(roles)).toContain('IotStatusRole');
  });

  it('gives only fotoko-http S3 object access, only under orders/', () => {
    const put = byAction('s3:PutObject');
    const { statement, roles } = one(put);
    expect(actionsOf(statement).sort()).toEqual([
      's3:GetObject',
      's3:PutObject',
    ]);
    expect(json(statement.Resource)).toMatch(
      /DataPhotosBucket\w+","Arn"\]},"\/orders\/\*"/,
    );
    expect(json(roles)).toContain('HttpRole');
    expect(byAction('s3:GetObject')).toHaveLength(1);
    expect(byAction('s3:DeleteObject')).toHaveLength(0);
  });

  it('lists the photos bucket (no condition: s3:prefix does not apply to HeadObject) so a missing key is a 404', () => {
    const { statement, roles } = one(byAction('s3:ListBucket'));
    expect(actionsOf(statement)).toEqual(['s3:ListBucket']);
    expect(json(statement.Resource)).toContain('DataPhotosBucket');
    expect(json(statement.Resource)).not.toContain('/*');
    expect(statement.Condition).toBeUndefined();
    expect(json(roles)).toContain('HttpRole');
  });

  it('allows iot:Publish only on printers jobs topics, only for fotoko-http', () => {
    const publish = byAction('iot:Publish').filter(({ roles }) =>
      json(roles).includes('Role'),
    );
    expect(publish).toHaveLength(1);
    const { statement, roles } = one(publish);
    expect(actionsOf(statement)).toEqual(['iot:Publish']);
    expect(json(statement.Resource)).toContain(':topic/fotoko/printers/*/jobs');
    expect(asArray(statement.Resource as unknown[])).toHaveLength(1);
    expect(json(roles)).toContain('HttpRole');
  });

  it('reads only the payOS secret, only for fotoko-http', () => {
    const secrets = byAction('secretsmanager:GetSecretValue');
    expect(secrets).toHaveLength(1);
    const { statement, roles } = one(secrets);
    expect(actionsOf(statement)).toEqual(['secretsmanager:GetSecretValue']);
    expect(json(statement.Resource)).toContain('DataPayosSecret');
    expect(json(roles)).toContain('HttpRole');
  });

  it('writes logs only to its own log group', () => {
    const logs = byAction('logs:PutLogEvents');
    // fotoko-http, fotoko-iot-status, IoT rule error logging, endpoint lookup provider.
    expect(logs).toHaveLength(4);
    for (const { statement } of logs) {
      expect(actionsOf(statement).sort()).toEqual([
        'logs:CreateLogStream',
        'logs:PutLogEvents',
      ]);
      expect(json(statement.Resource)).toContain('Logs');
    }
  });
});

describe('Cognito guest role', () => {
  it('may only invoke the four order routes of this API', () => {
    const invoke = byAction('execute-api:Invoke');
    expect(invoke).toHaveLength(1);
    const { statement } = one(invoke);
    expect(statement.Effect).toBe('Allow');
    expect(actionsOf(statement)).toEqual(['execute-api:Invoke']);
    const resources = asArray(statement.Resource as unknown[]).map((r) =>
      json(r),
    );
    expect(resources).toHaveLength(4);
    const tails = resources
      .map((r) => /\$default\/([^"]+)"/.exec(r)?.[1])
      .sort();
    expect(tails).toEqual([
      'GET/v1/orders/*',
      'POST/v1/orders',
      'POST/v1/orders/*/cancel',
      'POST/v1/orders/*/checkout',
    ]);
    for (const r of resources) expect(r).toContain('ApiHttpApi');
  });

  it('has no other permissions', () => {
    const guestPolicy = Object.values(
      template.findResources('AWS::IAM::Policy'),
    ).find((p) => json(p).includes('execute-api:Invoke'));
    expect(guestPolicy).toBeDefined();
    const statements = (
      guestPolicy?.Properties as { PolicyDocument: { Statement: unknown[] } }
    ).PolicyDocument.Statement;
    expect(statements).toHaveLength(1);
  });
});

describe('IoT device policy', () => {
  it('lets a Pi act only as its own thing', () => {
    const policy = Object.values(template.findResources('AWS::IoT::Policy'))[0];
    const statements = (
      policy?.Properties as {
        PolicyDocument: { Statement: Record<string, unknown>[] };
      }
    ).PolicyDocument.Statement;
    const arn = (resource: string): string =>
      `arn:aws:iot:ap-southeast-1:123456789012:${resource}`;
    const me = '${iot:Connection.Thing.ThingName}';
    const flat = statements.map((s) => ({
      action: actionsOf(s).join(','),
      resources: asArray(s.Resource as unknown[]).map(render),
    }));
    expect(flat).toEqual([
      { action: 'iot:Connect', resources: [arn(`client/${me}`)] },
      {
        action: 'iot:Publish',
        resources: [
          arn(`topic/fotoko/printers/${me}/status`),
          arn(`topic/fotoko/printers/${me}/heartbeat`),
        ],
      },
      {
        action: 'iot:Subscribe',
        resources: [arn(`topicfilter/fotoko/printers/${me}/jobs`)],
      },
      {
        action: 'iot:Receive',
        resources: [arn(`topic/fotoko/printers/${me}/jobs`)],
      },
    ]);
    expect(statements.every((s) => s.Effect === 'Allow')).toBe(true);
    expect(json(statements)).not.toMatch(/"\*"|topic\/\*|client\/\*/);
  });
});
