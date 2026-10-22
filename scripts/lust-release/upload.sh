#!/usr/bin/env bash
# upload.sh <file>: upload <file> as an unzipped GitHub Actions artifact named after the file,
# with the calls upload-artifact@v7 makes for `archive: false`: CreateArtifact, the blob as
# 8 MB blocks sent in parallel, FinalizeArtifact. The release job starts it before the build,
# so the create call runs during the build:
#   - the blob part waits for <file>.ready;
#   - the finalize waits for <file>.ok (the smoke passed), so a tsc that fails the smoke never
#     becomes an artifact.
# It stops when its parent step ends. Any failure exits non-zero, and the workflow then uploads
# with upload-artifact@v7. Needs ACTIONS_RUNTIME_TOKEN (only actions get it; the workflow's
# github-script step exports it) and ACTIONS_RESULTS_URL.
set -euo pipefail
f=$1 name=$(basename "$1")
: "${ACTIONS_RUNTIME_TOKEN:?}" "${ACTIONS_RESULTS_URL:?}"
wait_for() { while [ ! -e "$1" ]; do kill -0 "$PPID" 2>/dev/null || exit 1; sleep 0.01; done; }
# The backend ids are in the token's scp claim: "Actions.Results:<run id>:<job id>".
p=$(cut -d. -f2 <<< "$ACTIONS_RUNTIME_TOKEN" | tr '_-' '/+'); while (( ${#p} % 4 )); do p+='='; done
read -r run job < <(base64 -d <<< "$p" | jq -r .scp | tr ' ' '\n' | awk -F: '$1 == "Actions.Results" {print $2, $3}')
api() { curl -sS --fail-with-body --max-time 30 --retry 3 -H "Authorization: Bearer $ACTIONS_RUNTIME_TOKEN" -H 'Content-Type: application/json' \
  "${ACTIONS_RESULTS_URL%/}/twirp/github.actions.results.api.v1.ArtifactService/$1" -d "$2"; }
ids=$(jq -nc --arg r "$run" --arg j "$job" --arg n "$name" '{workflow_run_backend_id: $r, workflow_job_run_backend_id: $j, name: $n}')
url=$(api CreateArtifact "$(jq -c '. + {mime_type: "application/octet-stream", version: 7}' <<< "$ids")" | jq -er .signed_upload_url)
wait_for "$f.ready"
B=$(mktemp -d); trap 'rm -rf "$B"' EXIT
split -b 8M -d -a 3 "$f" "$B/b"
args=() xml='<?xml version="1.0" encoding="utf-8"?><BlockList>'
for b in "$B"/b*; do id=$(printf '%s' "${b##*/}" | base64); args+=(-T "$b" "$url&comp=block&blockid=$id"); xml+="<Latest>$id</Latest>"; done
curl -sS --fail-with-body --max-time 60 --retry 3 --parallel -H 'x-ms-version: 2023-11-03' "${args[@]}" > /dev/null
curl -sS --fail-with-body --max-time 30 --retry 3 -X PUT -H 'x-ms-version: 2023-11-03' -H 'x-ms-blob-content-type: application/octet-stream' \
  --data-binary "$xml</BlockList>" "$url&comp=blocklist" > /dev/null
size=$(stat -c %s "$f") hash=$(sha256sum "$f" | cut -d' ' -f1)
wait_for "$f.ok"
api FinalizeArtifact "$(jq -c --arg s "$size" --arg h "sha256:$hash" '. + {size: $s, hash: $h}' <<< "$ids")" | jq -er --arg s "$size" '"artifact \(.artifact_id), \($s) bytes"'
