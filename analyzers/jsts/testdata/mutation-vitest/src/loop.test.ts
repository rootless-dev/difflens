import { expect, test } from "vitest";

import { sumTo } from "./loop";

test("sums 1 to n", () => {
  expect(sumTo(3)).toBe(6);
  expect(sumTo(0)).toBe(0);
});
