"use strict";

function greet(name) {
  return `hello ${name || "world"}`;
}

const shout = function (name) {
  return greet(name).toUpperCase();
};

module.exports = { greet, shout };
