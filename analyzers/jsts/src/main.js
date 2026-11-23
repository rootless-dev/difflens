#!/usr/bin/env node
// Entry point: `node src/main.js <command> [flags]`. The exit code is set
// once stdout has been flushed, so a large document is never cut off.

import { main } from "./cli.js";

process.exitCode = await main(process.argv.slice(2), process.stdout, process.stderr);
