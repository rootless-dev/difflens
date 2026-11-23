import { describe, expect, it } from "vitest";

import { Circle, Square, totalArea } from "./shapes";

describe("totalArea", () => {
  it("adds the areas", () => {
    expect(totalArea([new Square(2), new Square(1)])).toBe(5);
  });

  it("is zero for nothing", () => {
    expect(totalArea([])).toBe(0);
  });
});

it("names a circle", () => {
  expect(new Circle(1).name).toBe("circle");
});
