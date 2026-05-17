#!/usr/bin/env bash
# Start RustFS and create the integration bucket. Idempotent.
#
# Reads no arguments; writes the test credentials into the env of any
# shell that sources `scripts/rustfs-env.sh` afterwards.
set -euo pipefail

cd "$(dirname "$0")/.."

if ! docker info >/dev/null 2>&1; then
  echo "docker daemon not reachable; start Docker Desktop and retry" >&2
  exit 2
fi

docker compose -f docker-compose.rustfs.yml up -d
echo "waiting for rustfs to be ready..."
for i in $(seq 1 30); do
  if curl -sS -o /dev/null http://localhost:19000/; then
    break
  fi
  sleep 1
done

export AWS_ACCESS_KEY_ID=testkey
export AWS_SECRET_ACCESS_KEY=testsecret
export AWS_REGION=us-east-1

if ! aws --endpoint-url http://localhost:19000 s3 ls s3://agentic-search-it >/dev/null 2>&1; then
  aws --endpoint-url http://localhost:19000 s3 mb s3://agentic-search-it
fi

cat > scripts/rustfs-env.sh <<'ENV'
# source this file to point the agentic-search CLI / tests at local RustFS.
export AWS_ACCESS_KEY_ID=testkey
export AWS_SECRET_ACCESS_KEY=testsecret
export AWS_REGION=us-east-1
export AWS_ENDPOINT_URL=http://localhost:19000
export AWS_VIRTUAL_HOSTED_STYLE_REQUEST=false
export AWS_ALLOW_HTTP=true
export RUSTFS_S3_TEST=1
ENV

echo "rustfs up; bucket agentic-search-it ready"
echo "source scripts/rustfs-env.sh in your shell to point at it"
