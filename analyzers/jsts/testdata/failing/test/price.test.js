import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { discount } from "../lib/price.js";

describe("discount", () => {
  it("keeps a zero discount", () => {
    assert.equal(discount(100, 0), 100);
  });

  it("takes ten percent off", () => {
    assert.equal(discount(100, 10), 90);
  });
});
