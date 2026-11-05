#!/usr/bin/env bash
# Deploy Fotoko to AWS: Lambda binaries, Expo web export and the CDK stack.
#
#   bash scripts/deploy/aws.sh [--dry-run] <dev|prod>
#
# Needs AWS credentials in the environment (CI: OIDC role), cargo-lambda and pnpm (CDK is in
# infra/cdk). With --dry-run every command is printed and nothing is run.
#
# The web build needs the API URL and identity pool id from the stack outputs. They come from
# EXPO_PUBLIC_API_URL / EXPO_PUBLIC_COGNITO_IDENTITY_POOL_ID when set, otherwise from the
# deployed stack. On the first deploy the stack does not exist yet, so it is deployed without
# the web assets first, then deployed again with them.
set -euo pipefail

DRY_RUN=0
STAGE=""
for arg in "$@"; do
    case "$arg" in
    --dry-run) DRY_RUN=1 ;;
    dev | prod) STAGE="$arg" ;;
    *)
        echo "usage: aws.sh [--dry-run] <dev|prod>" >&2
        exit 2
        ;;
    esac
done
[ -n "$STAGE" ] || {
    echo "usage: aws.sh [--dry-run] <dev|prod>" >&2
    exit 2
}

cd "$(git rev-parse --show-toplevel)"

REGION="${AWS_REGION:-ap-southeast-1}"
STACK="Fotoko-$STAGE"

# Print the command in dry-run mode, run it otherwise.
run() {
    if [ "$DRY_RUN" -eq 1 ]; then
        echo "+ $*"
    else
        echo "+ $*" >&2
        "$@"
    fi
}

# stack_output <OutputKey>: value from the deployed stack; empty when the stack or key is absent.
stack_output() {
    if [ "$DRY_RUN" -eq 1 ]; then
        echo "+ aws cloudformation describe-stacks --stack-name $STACK --region $REGION (output $1)" >&2
        echo "<$1>"
        return 0
    fi
    aws cloudformation describe-stacks --stack-name "$STACK" --region "$REGION" \
        --query "Stacks[0].Outputs[?OutputKey=='$1'].OutputValue" --output text 2>/dev/null |
        grep -v '^None$' || true
}

cdk_deploy() {
    # Run in the CDK package so `pnpm exec` finds its cdk binary; -c flags mirror infra/cdk/README.md.
    run pnpm --filter @fotoko/infra exec cdk deploy "$STACK" \
        -c "stage=$STAGE" -c "deployWeb=$1" --require-approval never
}

# 1. Lambda binaries (fotoko-api bins without the `local` feature).
if [ "$DRY_RUN" -eq 1 ]; then
    bash scripts/ci/lambda-build.sh --dry-run
else
    bash scripts/ci/lambda-build.sh
fi
if [ "$DRY_RUN" -eq 1 ]; then
    TARGET_DIR="<cargo target dir>"
else
    TARGET_DIR="$(cargo metadata --format-version 1 --no-deps | python3 -I -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')"
fi
export FOTOKO_LAMBDA_DIR="$TARGET_DIR/lambda"
export FOTOKO_STAGE="$STAGE"
[ "$DRY_RUN" -eq 0 ] || echo "+ export FOTOKO_LAMBDA_DIR=$FOTOKO_LAMBDA_DIR FOTOKO_STAGE=$STAGE"

# 2. Web build config: env first, then the deployed stack.
API_URL="${EXPO_PUBLIC_API_URL:-}"
POOL_ID="${EXPO_PUBLIC_COGNITO_IDENTITY_POOL_ID:-}"
if [ -z "$API_URL" ] || [ -z "$POOL_ID" ]; then
    API_URL="$(stack_output ApiUrl)"
    POOL_ID="$(stack_output IdentityPoolId)"
fi
if [ -z "$API_URL" ] || [ -z "$POOL_ID" ]; then
    echo "aws: stack $STACK has no outputs yet, deploying it without web assets first" >&2
    cdk_deploy false
    API_URL="$(stack_output ApiUrl)"
    POOL_ID="$(stack_output IdentityPoolId)"
fi
[ -n "$API_URL" ] && [ -n "$POOL_ID" ] || {
    echo "aws: ERROR: no API URL or identity pool id; set EXPO_PUBLIC_API_URL and EXPO_PUBLIC_COGNITO_IDENTITY_POOL_ID" >&2
    exit 1
}

# 3. Expo web export (apps/mobile/dist), configured for aws mode.
run env EXPO_PUBLIC_FOTOKO_MODE=aws \
    "EXPO_PUBLIC_API_URL=$API_URL" \
    "EXPO_PUBLIC_AWS_REGION=$REGION" \
    "EXPO_PUBLIC_COGNITO_IDENTITY_POOL_ID=$POOL_ID" \
    pnpm --filter @fotoko/mobile build:web

# 4. Deploy the stack with the web assets.
cdk_deploy true

# 5. Outputs.
if [ "$DRY_RUN" -eq 1 ]; then
    echo "+ aws cloudformation describe-stacks --stack-name $STACK --region $REGION --query Stacks[0].Outputs --output table"
else
    aws cloudformation describe-stacks --stack-name "$STACK" --region "$REGION" \
        --query 'Stacks[0].Outputs' --output table
fi
