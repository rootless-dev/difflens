export const config = {
  load(name) {
    return name ?? "default";
  },
};

export default function main(argv) {
  return argv.map((arg) => config.load(arg));
}
