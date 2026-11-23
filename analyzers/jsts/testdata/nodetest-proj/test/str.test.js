import assert from "node:assert/strict";
import { describe, it, test } from "node:test";

import { isPalindrome, reverse } from "../src/str.js";

describe("reverse", () => {
  it("reverses", () => {
    assert.equal(reverse("abc"), "cba");
  });

  it("keeps empty", () => {
    assert.equal(reverse(""), "");
  });
});

test("palindromes", async (t) => {
  await t.test("racecar", () => {
    assert.ok(isPalindrome("Racecar"));
  });
  await t.test("not hello", () => {
    assert.ok(!isPalindrome("hello"));
  });
});

test("skipped one", { skip: true }, () => {});
