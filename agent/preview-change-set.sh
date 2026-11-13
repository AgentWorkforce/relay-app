#!/usr/bin/env bash
# Agent Deploy's preview (.github/workflows/agent-deploy.yml, preview job):
# before the deploy action runs, create a NON-EXECUTING CloudFormation
# change set on the compute stack from the template the AMI pin adoption
# would deploy (AmiId pinned plus AgentHost's LayeredAgentAmiPin metadata
# marker, as kit 0.1.15 builds it), and fail unless AgentHost is modified in
# place and AmiId takes the template's pinned default (a replacement is allowed
# only with PREVIEW_ALLOW_REPLACEMENT=true, set for an allow_restart dispatch;
# see preview-change-set.ts assert). A change set with no changes fails: the
# pin would not persist. Read-only on the host: it never runs the kit's
# decide (which hot-updates the host over SSM) and never executes the change
# set.
#
# A stack that verifiably does not exist yet (the first deploy of a fresh
# agent) passes without a change set: there is no host to replace, and a fresh
# create is what the preview protects. Only a ValidationError saying "does not
# exist" counts, never one that is also an authorization error; any other
# get-template error is printed and fails the run. The run step's output
# (GITHUB_OUTPUT) carries fresh=true on that path and fresh=false once the
# deployed template was read: the workflow deploys a fresh create only from an
# allow_restart dispatch, since the kit treats the first create as a full
# deploy. Without PREVIEW_ALLOW_REPLACEMENT=true the fresh-create path also
# prints a ::notice:: saying this run deploys nothing and an operator must
# dispatch Agent Deploy with allow_restart to create the stack.
#
#   preview-change-set.sh run <change-set-name>
#   preview-change-set.sh delete <change-set-name>
#     exits 0 when the change set was deleted or verifiably does not exist
#     (ChangeSetNotFound / "does not exist": never created, or already gone);
#     any other error (AccessDenied, network, ...) is printed and exits 1, so
#     the always() delete step fails the preview job and the deploy never runs
#
# AWS calls (the deploy role's credentials): cloudformation GetTemplate,
# DescribeStacks, CreateChangeSet, DescribeChangeSet, DeleteChangeSet on the
# compute stack. Any other error fails the run (fail closed). The JSON checks
# live in preview-change-set.ts (tested in test/preview-change-set.test.ts).
set -euo pipefail
cd "$(dirname "$0")"

mode=${1:-}
name=${2:-}
if [[ ! "$mode" =~ ^(run|delete)$ || ! "$name" =~ ^[A-Za-z][-A-Za-z0-9]{0,127}$ ]]; then
  echo "usage: preview-change-set.sh run|delete <change-set-name>" >&2
  exit 64
fi

where=$(node preview-change-set.ts stack)
stack=${where% *}
region=${where#* }
cfn=(aws cloudformation --region "$region" --output json)
summary=${GITHUB_STEP_SUMMARY:-/dev/null}
output=${GITHUB_OUTPUT:-/dev/null}

if [ "$mode" = delete ]; then
  if err=$("${cfn[@]}" delete-change-set --stack-name "$stack" --change-set-name "$name" 2>&1 >/dev/null); then
    echo "deleted change set $name on $stack"
    exit 0
  fi
  # Only a verified not-found is success; an authorization error never is,
  # whatever else its message says.
  if [[ "$err" =~ ChangeSetNotFound|does\ not\ exist ]] && [[ ! "$err" =~ AccessDenied|not\ authorized ]]; then
    echo "change set $name on $stack does not exist (never created, or already gone)"
    exit 0
  fi
  echo "could not delete change set $name on $stack:" >&2
  printf '%s\n' "$err" >&2
  exit 1
fi

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

if ! err=$("${cfn[@]}" get-template --stack-name "$stack" --template-stage Original --query TemplateBody 2>&1 >"$work/deployed.json"); then
  # Only a verified not-found ("Stack with id <name> does not exist") is a
  # fresh create; an authorization error never is, whatever else it says.
  if [[ "$err" =~ ValidationError ]] && [[ "$err" =~ does\ not\ exist ]] && [[ ! "$err" =~ AccessDenied|not\ authorized ]]; then
    echo "stack $stack does not exist yet: a fresh create, nothing to replace"
    echo "Agent Deploy preview: stack $stack does not exist yet, fresh create, nothing to replace: PASS" >>"$summary"
    echo "fresh=true" >>"$output"
    if [ "${PREVIEW_ALLOW_REPLACEMENT:-}" != true ]; then
      echo "::notice::Stack $stack does not exist yet. Its first create is a full deploy, so this run deploys nothing; an operator must dispatch Agent Deploy with allow_restart to create it."
    fi
    exit 0
  fi
  echo "could not read the deployed template of $stack:" >&2
  printf '%s\n' "$err" >&2
  exit 1
fi
echo "fresh=false" >>"$output"
"${cfn[@]}" describe-stacks --stack-name "$stack" >"$work/stacks.json"
kind=$(node preview-change-set.ts template "$work/deployed.json" "$work/stacks.json" "$work/pin.json")
if [ "$kind" = adopted ]; then
  echo "pin already adopted on $stack, nothing to preview"
  echo "Agent Deploy preview: $stack already carries the AMI pin, nothing to preview: PASS" >>"$summary"
  exit 0
fi

mapfile -t params < <(node preview-change-set.ts params "$work/pin.json")
[ "${#params[@]}" -gt 0 ] || { echo "the pin template has no parameters to keep" >&2; exit 1; }
# AmiId is left out on purpose: Alchemy does not pass it, so the update takes
# the template default, which is what this preview must show.
"${cfn[@]}" create-change-set --stack-name "$stack" --change-set-name "$name" \
  --change-set-type UPDATE --template-body "file://$work/pin.json" \
  --parameters "${params[@]}" >/dev/null
echo "created change set $name on $stack (not executed)"
# A change set with no changes ends FAILED, which fails the waiter; the
# describe below and the assert decide (and fail it: the pin would not persist).
"${cfn[@]}" wait change-set-create-complete --stack-name "$stack" --change-set-name "$name" \
  || echo "change set $name did not reach CREATE_COMPLETE; checking why"
"${cfn[@]}" describe-change-set --stack-name "$stack" --change-set-name "$name" >"$work/change-set.json"
if line=$(node preview-change-set.ts assert "$work/change-set.json" "$work/pin.json"); then
  echo "$line"
  echo "Agent Deploy preview: stack $stack, change set $name, $line" >>"$summary"
else
  echo "$line"
  echo "Agent Deploy preview: stack $stack, change set $name, ${line:-FAIL}" >>"$summary"
  exit 1
fi
