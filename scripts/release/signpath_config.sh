#!/usr/bin/env bash
# All configuration absent means the pending-certificate integration is inactive.
set -euo pipefail
configured=0
for name in SIGNPATH_API_TOKEN SIGNPATH_ORGANIZATION_ID SIGNPATH_SIGNER_SUBJECT; do
  [[ -z "${!name:-}" ]] || configured=$((configured + 1))
done
if (( configured != 0 && configured != 3 )); then
  echo '::error::Complete the SignPath configuration in .github/RELEASE.md or leave all three values unset.'
  exit 1
fi
if (( configured == 3 )); then
  echo 'enabled=true' >> "${GITHUB_OUTPUT}"
else
  echo 'enabled=false' >> "${GITHUB_OUTPUT}"
  echo '::notice::SignPath is inactive: certificate/setup pending. Windows drafts remain unsigned.'
fi
