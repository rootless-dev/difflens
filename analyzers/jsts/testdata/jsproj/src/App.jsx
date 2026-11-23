export function Button({ onClick, label }) {
  return <button onClick={onClick}>{label}</button>;
}

export default function App() {
  const handleClick = () => {
    console.log("clicked");
  };
  return (
    <div>
      <Button onClick={handleClick} label="go" />
    </div>
  );
}
