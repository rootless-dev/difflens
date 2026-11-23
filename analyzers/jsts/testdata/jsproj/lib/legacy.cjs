function Counter() {
  this.value = 0;
}

Counter.prototype.increment = function () {
  this.value++;
  return this;
};

module.exports = Counter;
