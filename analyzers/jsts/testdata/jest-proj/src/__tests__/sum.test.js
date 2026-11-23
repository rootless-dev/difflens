const { sum, average } = require("../sum");

describe("sum", () => {
  test("adds", () => {
    expect(sum(1, 2, 3)).toBe(6);
  });

  test("is zero for nothing", () => {
    expect(sum()).toBe(0);
  });
});

test("averages", () => {
  expect(average(2, 4)).toBe(3);
});
