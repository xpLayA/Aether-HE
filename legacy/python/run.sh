#!/usr/bin/env bash
# Launch the legacy Python app (reference implementation). Uses the repo-root venv.
cd "$(dirname "$0")"
exec ../../venv-web/bin/python app_web.py "$@"
