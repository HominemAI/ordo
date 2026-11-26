#!/usr/bin/env bash
# Only the disposable CI object store. Uses the same pinned images as ci.yml.
set -euo pipefail
name=ordo-marketing-s3
case "${1:-start}" in
  start)
    docker run -d --name "$name" -p 127.0.0.1:8333:8333 -p 127.0.0.1:9333:9333 \
      -e AWS_ACCESS_KEY_ID=ordo-ci-key -e AWS_SECRET_ACCESS_KEY=ordo-ci-secret-not-a-secret \
      chrislusf/seaweedfs:4.09 server -s3 -dir=/data
    for attempt in $(seq 1 60); do
      curl -fsS http://127.0.0.1:9333/cluster/status >/dev/null && break
      sleep 1
    done
    curl -fsS http://127.0.0.1:9333/cluster/status >/dev/null
    for attempt in $(seq 1 30); do
      docker run --rm --network host -e AWS_ACCESS_KEY_ID=ordo-ci-key \
        -e AWS_SECRET_ACCESS_KEY=ordo-ci-secret-not-a-secret -e AWS_DEFAULT_REGION=us-east-1 \
        amazon/aws-cli:2.32.29 --endpoint-url http://127.0.0.1:8333 \
        s3api create-bucket --bucket marketing-attachments && break
      sleep 2
    done
    docker run --rm --network host -e AWS_ACCESS_KEY_ID=ordo-ci-key \
      -e AWS_SECRET_ACCESS_KEY=ordo-ci-secret-not-a-secret -e AWS_DEFAULT_REGION=us-east-1 \
      amazon/aws-cli:2.32.29 --endpoint-url http://127.0.0.1:8333 \
      s3api head-bucket --bucket marketing-attachments
    ;;
  stop) docker rm -f "$name" ;;
  *) echo 'usage: s3-fixture.sh start|stop' >&2; exit 1 ;;
esac
