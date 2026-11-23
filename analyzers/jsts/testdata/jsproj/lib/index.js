const { greet } = require("./greet");
const { Ticker } = require("./events");

exports.greet = greet;

exports.startTicker = function startTicker(interval) {
  const ticker = new Ticker();
  ticker.on("tick", onTick);
  ticker.start(interval);
  return ticker;
};

function onTick(count) {
  if (count > 3) {
    return;
  }
  greet(String(count));
}
