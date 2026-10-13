#!/usr/bin/env bash
# Quiet: the page, and nothing more: the desktop does not change.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
material
browser
ready
until_end
close chromium
