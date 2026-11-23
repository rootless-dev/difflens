export function reverse(text) {
  return [...text].reverse().join("");
}

export function isPalindrome(text) {
  const clean = text.toLowerCase().replace(/[^a-z]/g, "");
  return clean.length > 0 && clean === reverse(clean);
}

export function never() {
  return "not covered";
}
