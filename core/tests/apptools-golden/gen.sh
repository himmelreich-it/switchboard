#!/bin/sh
# Regenerates core/tests/apptools-tools-list.json and apptools-protocol.json from the real apptools.ts, MCP SDK and
# zod: apptools.ts is copied next to stub.ts (stand-ins for server.ts, runner.ts, agents.ts) and harness.ts calls
# its handleMcp. Run from anywhere: sh core/tests/apptools-golden/gen.sh
set -e
here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../../.." && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
cp "$here/stub.ts" "$here/harness.ts" "$work/"
ln -s "${NODE_MODULES:-$repo/node_modules}" "$work/node_modules"
sed -e "s#from \"./lib\"#from \"$repo/lib.ts\"#" \
    -e 's#from "./server"#from "./stub"#' -e 's#from "./runner"#from "./stub"#' \
    -e 's#from "./agents"#from "./stub"#' -e 's#from "./acp"#from "./stub"#' \
    "$repo/apptools.ts" > "$work/apptools.ts"
bun "$work/harness.ts" "$here/.."
