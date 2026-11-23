function sum(...values) {
  return values.reduce((total, value) => total + value, 0);
}

function average(...values) {
  if (values.length === 0) {
    return 0;
  }
  return sum(...values) / values.length;
}

module.exports = { sum, average };
