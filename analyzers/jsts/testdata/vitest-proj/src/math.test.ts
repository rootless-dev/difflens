import { describe, expect, it, test } from "vitest";

import { add, divide } from "./math";

describe("math", () => {
  it("adds", () => {
    expect(add(1, 2)).toBe(3);
  });

  describe("divide", () => {
    it("divides", () => {
      expect(divide(6, 3)).toBe(2);
    });

    it("refuses zero", () => {
      expect(() => divide(1, 0)).toThrow("division by zero");
    });
  });

  it.skip("is skipped", () => {
    expect(true).toBe(false);
  });
});

test("adds negatives / with a slash", () => {
  expect(add(-1, -2)).toBe(-3);
});
