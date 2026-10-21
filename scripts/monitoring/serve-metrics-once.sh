#!/usr/bin/env bash
# Writes one HTTP response containing the current metrics page.
# Invoked per connection by socat from prometheus-exporter.sh, which passes
# the metrics path in the METRICS_FILE environment variable.
printf 'HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nConnection: close\r\n\r\n'
cat "${METRICS_FILE:?METRICS_FILE is not set}"
