// JSON line logs on stderr, as every DiffLens analyzer writes them: one
// object per line with `time`, `level`, `service` and `msg`, then the fields
// given. The level comes from DIFFLENS_LOG_LEVEL; an unknown name falls back
// to info, because losing logs is worse than logging too much.

const LEVELS = { trace: 10, debug: 10, info: 20, warn: 30, warning: 30, error: 40 };

export function createLogger(stream, service, level) {
  const threshold = LEVELS[String(level ?? "").trim().toLowerCase()] ?? LEVELS.info;
  const write = (name, msg, fields = {}) => {
    if (LEVELS[name] < threshold) return;
    stream.write(`${JSON.stringify({ time: new Date().toISOString(), level: name, service, msg, ...fields })}\n`);
  };
  return {
    debug: (msg, fields) => write("debug", msg, fields),
    info: (msg, fields) => write("info", msg, fields),
    warn: (msg, fields) => write("warn", msg, fields),
    error: (msg, fields) => write("error", msg, fields),
  };
}
