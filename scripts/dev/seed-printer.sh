#!/usr/bin/env bash
# Create the Printer item of a Stand in the DynamoDB table (the API refuses orders for printers
# it does not know). The item format is the one crates/api/src/aws/dynamo.rs reads
# (docs/contracts.md "DynamoDB"): pk PRINTER#<id>, sk META, name, pricePerPrintVnd, status.
# The Pi's first heartbeat flips `status` to online; the item must exist before that.
#
#   bash scripts/dev/seed-printer.sh --printer-id stand-001 --stage dev [--name "Stand 1"] \
#        [--price 10000] [--region ap-southeast-1] [--force] [--dry-run]
#
# Needs AWS credentials with dynamodb:PutItem on table fotoko-<stage>. Without --force an
# existing item is left alone (the put fails its attribute_not_exists condition).
set -euo pipefail

usage() {
    sed -n '2,/^set -euo/p' "${BASH_SOURCE[0]}" | sed '$d; s/^# \{0,1\}//' >&2
    exit 2
}
die() {
    echo "seed-printer: ERROR: $*" >&2
    exit 1
}

printer_id=""
stage=""
name=""
price=10000
region="${AWS_REGION:-ap-southeast-1}"
force=0
dry_run=0
while [ $# -gt 0 ]; do
    case "$1" in
    --printer-id) printer_id="${2:?--printer-id needs a value}"; shift 2 ;;
    --stage) stage="${2:?--stage needs a value}"; shift 2 ;;
    --name) name="${2:?--name needs a value}"; shift 2 ;;
    --price) price="${2:?--price needs a value}"; shift 2 ;;
    --region) region="${2:?--region needs a value}"; shift 2 ;;
    --force) force=1; shift ;;
    --dry-run) dry_run=1; shift ;;
    -h | --help) usage ;;
    *) usage ;;
    esac
done

[ -n "$printer_id" ] && [ -n "$stage" ] || usage
case "$stage" in dev | prod) ;; *) die "--stage must be dev or prod" ;; esac
# Same rule as PrinterId (crates/protocol) plus IoT thing names (infra provision-printer).
[[ $printer_id =~ ^[A-Za-z0-9:_-]{1,128}$ ]] || die "printer id must match [A-Za-z0-9:_-]{1,128}: $printer_id"
[[ $price =~ ^[1-9][0-9]*$ ]] || die "--price must be a positive integer (VND): $price"
[ -n "$name" ] || name="$printer_id"

command -v jq >/dev/null 2>&1 || die "jq not found"
table="fotoko-$stage"
item=$(jq -nc --arg pk "PRINTER#$printer_id" --arg name "$name" --arg price "$price" \
    '{pk:{S:$pk}, sk:{S:"META"}, name:{S:$name}, pricePerPrintVnd:{N:$price}, status:{S:"offline"}}')

cmd=(aws dynamodb put-item --region "$region" --table-name "$table" --item "$item")
[ "$force" -eq 1 ] || cmd+=(--condition-expression "attribute_not_exists(pk)")

if [ "$dry_run" -eq 1 ]; then
    printf '+'
    printf ' %q' "${cmd[@]}"
    echo
    exit 0
fi
command -v aws >/dev/null 2>&1 || die "aws CLI not found (https://docs.aws.amazon.com/cli/latest/userguide/getting-started-install.html)"
if ! "${cmd[@]}"; then
    die "put-item failed (an existing item needs --force; check credentials and table $table)"
fi
echo "seed-printer: $printer_id in $table ($region), price $price VND, status offline until the Pi's first heartbeat"
