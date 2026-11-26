#!/usr/bin/env bash
# Writes one HTTP response containing the current metrics page.
# Invoked per connection by socat from prometheus-exporter.sh, which passes
# the metrics path in the METRICS_FILE environment variable.
#
# A failed collection must never look like a successful scrape: if
# METRICS_FILE is unset, missing or unreadable the script exits non-zero
# without writing any part of an HTTP 200 response.
set -euo pipefail

metrics_file="${METRICS_FILE:?METRICS_FILE is not set}"

# Read the whole page before writing the status line. Once "200 OK" is on the
# wire the scraper treats the response as a success even if the body is lost.
# The trailing sentinel keeps the body byte-exact (command substitution would
# otherwise drop trailing newlines). It is chained with && so a failed read
# fails the substitution, and set -e then exits before any header is written.
body="$(cat -- "$metrics_file" && printf x)"
body="${body%x}"

printf 'HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nConnection: close\r\n\r\n'
printf '%s' "$body"
