class Stack {
  constructor() {
    this.items = [];
  }

  push(item) {
    this.items.push(item);
    return this;
  }

  pop() {
    if (this.items.length === 0) {
      throw new Error("empty stack");
    }
    return this.items.pop();
  }
}

module.exports = { Stack };
