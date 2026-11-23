import { expect, test } from "vitest";

import { upper } from "./upper";

test("upper", () => {
  expect(upper("a")).toBe("A");
});

test("fails on purpose", () => {
  expect(upper("a")).toBe("B");
});
