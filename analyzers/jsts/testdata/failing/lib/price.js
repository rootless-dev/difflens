export function discount(price, percent) {
  // Bug on purpose: the discount is added instead of subtracted.
  return price + (price * percent) / 100;
}
