export function report(items) {
  const unused = 1;
  if (items == null) {
    return undefinedThing;
  }
  console.log(items.length);
  return items.length;
}
