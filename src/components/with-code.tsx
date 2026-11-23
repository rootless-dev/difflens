/** Text with `code` in backticks shown as code; never read as HTML. */
export function WithCode({ text }: { text: string }) {
  const parts = text.split("`");
  // An odd number of backticks leaves the last one as written.
  if (parts.length % 2 === 0) return <>{text}</>;
  return (
    <>
      {parts.map((part, index) => (index % 2 === 1 ? <code key={index}>{part}</code> : <span key={index}>{part}</span>))}
    </>
  );
}
