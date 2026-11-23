import { describe, expect, it } from "vitest";

import { canWithdraw, deposit } from "./account";

describe("account", () => {
  it("allows a withdrawal below the balance", () => {
    expect(canWithdraw(100, 50)).toBe(true);
  });

  it("refuses a withdrawal above the balance", () => {
    expect(canWithdraw(10, 50)).toBe(false);
  });

  it("adds a deposit", () => {
    expect(deposit(100, 50)).toBe(150);
  });

  it("refuses an empty deposit", () => {
    expect(() => deposit(100, 0)).toThrow("deposit must be positive");
  });
});
