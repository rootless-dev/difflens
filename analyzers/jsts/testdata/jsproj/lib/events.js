const { EventEmitter } = require("node:events");

class Ticker extends EventEmitter {
  constructor() {
    super();
    this.count = 0;
  }

  start(interval) {
    this.timer = setInterval(() => this.tick(), interval);
  }

  tick() {
    this.count += 1;
    this.emit("tick", this.count);
    if (this.count >= 10) {
      clearInterval(this.timer);
    }
  }
}

function load(path) {
  return fetchText(path)
    .then((text) => text.trim())
    .catch(() => "");
}

async function fetchText(path) {
  return `contents of ${path}`;
}

module.exports = { Ticker, load };
