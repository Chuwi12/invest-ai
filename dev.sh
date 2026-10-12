#!/usr/bin/env bash
# Atajo histórico: equivale a ./invest-ai dev
set -Eeuo pipefail
exec "$(dirname -- "$(readlink -f -- "${BASH_SOURCE[0]}")")/invest-ai" dev "$@"
