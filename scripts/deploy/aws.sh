#!/usr/bin/env bash
# Deploy Fotoko to AWS in four phases. Only deploy-infra and deploy-web need AWS credentials;
# the build phases run without them (CI runs them as separate jobs).
#
#   bash scripts/deploy/aws.sh [--dry-run] <command> [dev|prod]
#
#   build-lambdas        cargo lambda build for the fotoko-api bins (no credentials, no stage)
#   deploy-infra <stage> cdk deploy without web assets; prints api_url= and pool_id=
#   build-web <stage>    Expo web export; reads EXPO_PUBLIC_API_URL and
#                        EXPO_PUBLIC_COGNITO_IDENTITY_POOL_ID from the environment, else from the
#                        deployed stack (needs credentials then)
#   deploy-web <stage>   cdk deploy with the web assets (apps/mobile/dist), prints stack outputs
#   all <stage>          the four phases in order
#
# Lambda binaries are read from FOTOKO_LAMBDA_DIR (default <cargo target dir>/lambda).
# --dry-run prints every command and runs none.
set -euo pipefail

usage() {
    echo "usage: aws.sh [--dry-run] <build-lambdas|deploy-infra|build-web|deploy-web|all> [dev|prod]" >&2
    exit 2
}

DRY_RUN=0
CMD=""
STAGE=""
for arg in "$@"; do
    case "$arg" in
    --dry-run) DRY_RUN=1 ;;
    build-lambdas | deploy-infra | build-web | deploy-web | all) CMD="$arg" ;;
    dev | prod) STAGE="$arg" ;;
    *) usage ;;
    esac
done
[ -n "$CMD" ] || usage
if [ "$CMD" != build-lambdas ] && [ -z "$STAGE" ]; then
    usage
fi

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

target_dir() {
    if [ "$DRY_RUN" -eq 1 ]; then
        echo "<cargo target dir>"
    else
        cargo metadata --format-version 1 --no-deps | python3 -I -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])'
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

# CDK reads the Lambda binaries from FOTOKO_LAMBDA_DIR. Artifact downloads drop the execute bit.
use_lambda_dir() {
    export FOTOKO_LAMBDA_DIR="${FOTOKO_LAMBDA_DIR:-$(target_dir)/lambda}"
    export FOTOKO_STAGE="$STAGE"
    if [ "$DRY_RUN" -eq 1 ]; then
        echo "+ export FOTOKO_LAMBDA_DIR=$FOTOKO_LAMBDA_DIR FOTOKO_STAGE=$STAGE"
        echo "+ chmod +x $FOTOKO_LAMBDA_DIR/*/bootstrap"
    else
        chmod +x "$FOTOKO_LAMBDA_DIR"/*/bootstrap
    fi
}

cdk_deploy() { # cdk_deploy <deployWeb true|false>
    use_lambda_dir
    run pnpm --filter @fotoko/infra exec cdk deploy "$STACK" \
        -c "stage=$STAGE" -c "deployWeb=$1" --require-approval never
}

build_lambdas() {
    if [ "$DRY_RUN" -eq 1 ]; then
        bash scripts/ci/lambda-build.sh --dry-run
    else
        bash scripts/ci/lambda-build.sh
    fi
}

deploy_infra() {
    cdk_deploy false
    local api_url pool_id
    api_url="$(stack_output ApiUrl)"
    pool_id="$(stack_output IdentityPoolId)"
    echo "api_url=$api_url"
    echo "pool_id=$pool_id"
    if [ -n "${GITHUB_OUTPUT:-}" ] && [ "$DRY_RUN" -eq 0 ]; then
        printf 'api_url=%s\npool_id=%s\n' "$api_url" "$pool_id" >>"$GITHUB_OUTPUT"
    fi
}

build_web() {
    local api_url="${EXPO_PUBLIC_API_URL:-}"
    local pool_id="${EXPO_PUBLIC_COGNITO_IDENTITY_POOL_ID:-}"
    if [ -z "$api_url" ] || [ -z "$pool_id" ]; then
        api_url="$(stack_output ApiUrl)"
        pool_id="$(stack_output IdentityPoolId)"
    fi
    if [ -z "$api_url" ] || [ -z "$pool_id" ]; then
        echo "aws: ERROR: no API URL or identity pool id; run deploy-infra first or set EXPO_PUBLIC_API_URL and EXPO_PUBLIC_COGNITO_IDENTITY_POOL_ID" >&2
        exit 1
    fi
    run env EXPO_PUBLIC_FOTOKO_MODE=aws \
        "EXPO_PUBLIC_API_URL=$api_url" \
        "EXPO_PUBLIC_AWS_REGION=$REGION" \
        "EXPO_PUBLIC_COGNITO_IDENTITY_POOL_ID=$pool_id" \
        pnpm --filter @fotoko/mobile build:web
}

deploy_web() {
    if [ "$DRY_RUN" -eq 0 ] && [ ! -d apps/mobile/dist ]; then
        echo "aws: ERROR: apps/mobile/dist is missing; run build-web first" >&2
        exit 1
    fi
    cdk_deploy true
    if [ "$DRY_RUN" -eq 1 ]; then
        echo "+ aws cloudformation describe-stacks --stack-name $STACK --region $REGION --query Stacks[0].Outputs --output table"
    else
        aws cloudformation describe-stacks --stack-name "$STACK" --region "$REGION" \
            --query 'Stacks[0].Outputs' --output table
    fi
}

case "$CMD" in
build-lambdas) build_lambdas ;;
deploy-infra) deploy_infra ;;
build-web) build_web ;;
deploy-web) deploy_web ;;
all)
    build_lambdas
    deploy_infra
    build_web
    deploy_web
    ;;
esac
