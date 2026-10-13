#!/usr/bin/env bash
# Quiet: the page, and nothing more. Opened before the session that captures
# it (run.sh), so that the session's keyframe is the page already drawn and
# the desktop does not change after it.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
material
browser
until_end
close chromium
