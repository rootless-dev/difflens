"use strict";

const { readFileSync, writeFileSync, existsSync } = require("node:fs");
const { join, dirname, basename } = require("node:path");
const { promisify, inspect } = require("node:util");
const { setTimeout: sleep } = require("node:timers/promises");
const assert = require("node:assert");
const events = require("node:events").EventEmitter;
require("./polyfills.cjs");

async function retry(task, attempts, delayMs) {
  let lastError = null;
  for (let attempt = 1; attempt <= attempts; attempt++) {
    try {
      return await task(attempt);
    } catch (error) {
      lastError = error;
      await sleep(delayMs * attempt);
    }
  }
  throw new Error(`gave up after ${attempts} attempts: ${lastError}`);
}

module.exports = { retry, readFileSync, writeFileSync, existsSync, join, dirname, basename, promisify, inspect, assert, events };
