"use strict";

const { readFileSync, writeFileSync, existsSync } = require("node:fs");
const { join, dirname, basename } = require("node:path");
const { promisify, inspect } = require("node:util");
const { setTimeout: sleep } = require("node:timers/promises");
const assert = require("node:assert");
const events = require("node:events").EventEmitter;
require("./polyfills.cjs");

function describeFile(path) {
  return existsSync(path) ? `${basename(path)} in ${dirname(path)}` : "missing";
}

module.exports = { describeFile, sleep, readFileSync, writeFileSync, join, promisify, inspect, assert, events };
