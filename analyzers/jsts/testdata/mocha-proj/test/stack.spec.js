const assert = require("node:assert");

const { Stack } = require("../lib/stack");

describe("Stack", function () {
  it("pops what was pushed", function () {
    assert.strictEqual(new Stack().push(1).pop(), 1);
  });

  context("when empty", function () {
    it("throws", function () {
      assert.throws(() => new Stack().pop(), /empty/);
    });

    it("fails on purpose", function () {
      assert.strictEqual(new Stack().push(2).pop(), 3);
    });
  });
});
