#!/bin/sh
while true; do
    /usr/local/bin/reference-peer "$@"
    EXIT_CODE=$?
    if [ "$EXIT_CODE" -ne 42 ]; then
        exit $EXIT_CODE
    fi
    DELAY=$(cat /tmp/delay 2>/dev/null || echo "0")
    redis-cli -u "$REDIS_URL" LPUSH "${PEER_NAME}_log" "info|Restarting in ${DELAY} seconds"
    sleep "$DELAY"
done
