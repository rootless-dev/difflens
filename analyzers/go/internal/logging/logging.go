// Package logging keeps the logging library behind a small interface so the
// rest of the analyzer does not depend on it.
package logging

import (
	"io"
	"strings"

	"github.com/phuslu/log"
)

// Logger is the only logging surface the analyzer uses. Key-value pairs follow
// the message, as in Info("loaded", "packages", 12).
type Logger interface {
	Debug(msg string, keysAndValues ...any)
	Info(msg string, keysAndValues ...any)
	Warn(msg string, keysAndValues ...any)
	Error(msg string, keysAndValues ...any)
}

// New returns a JSON line logger writing to w. An unknown level name falls
// back to info: losing logs is worse than logging too much.
func New(w io.Writer, service, level string) Logger {
	log.MessageKey = "msg"
	return &phusluLogger{l: log.Logger{
		Level:   parseLevel(level),
		Context: log.NewContext(nil).Str("service", service).Value(),
		Writer:  &log.IOWriter{Writer: w},
	}}
}

// Discard returns a logger that drops everything.
func Discard() Logger { return New(io.Discard, "", "error") }

func parseLevel(name string) log.Level {
	switch strings.ToLower(strings.TrimSpace(name)) {
	case "trace", "debug":
		return log.DebugLevel
	case "warn", "warning":
		return log.WarnLevel
	case "error":
		return log.ErrorLevel
	default:
		return log.InfoLevel
	}
}

type phusluLogger struct{ l log.Logger }

func (p *phusluLogger) Debug(msg string, kv ...any) { p.l.Debug().KeysAndValues(kv...).Msg(msg) }
func (p *phusluLogger) Info(msg string, kv ...any)  { p.l.Info().KeysAndValues(kv...).Msg(msg) }
func (p *phusluLogger) Warn(msg string, kv ...any)  { p.l.Warn().KeysAndValues(kv...).Msg(msg) }
func (p *phusluLogger) Error(msg string, kv ...any) { p.l.Error().KeysAndValues(kv...).Msg(msg) }
