package dupes

import (
	"go/scanner"
	"go/token"
	"strings"
)

// tok is one normalized token of a file, 16 bytes: a large tree has
// millions of them.
type tok struct {
	Kind  kind
	Start int32 // line the token starts on
	End   int32 // line it ends on
	Split int32 // the run of tokens between left-out declarations it belongs to
}

// kind is a go/token.Token, which all fit in a byte. token.IDENT,
// token.STRING and token.INT stand for every identifier, string or
// character, and number.
type kind uint8

func (k kind) String() string { return token.Token(k).String() }

// What the tokenizer is reading.
const (
	inCode           = iota
	inClause         // the package clause or a single import spec, through its ";"
	afterImport      // right after the import keyword
	inImportGroup    // between the "(" and ")" of an import declaration
	afterImportGroup // right after that ")", whose ";" ends the declaration
)

// tokenize returns the normalized tokens of src, comments dropped and the
// package clause and import declarations left out. Each left-out
// declaration starts a new split. A lexical error fails the whole file.
func tokenize(src []byte) ([]tok, error) {
	fset := token.NewFileSet()
	file := fset.AddFile("", fset.Base(), len(src))
	var errs scanner.ErrorList
	var s scanner.Scanner
	s.Init(file, src, func(pos token.Position, msg string) { errs.Add(pos, msg) }, 0)

	var toks []tok
	split := int32(0)
	state := inCode
	for {
		pos, t, lit := s.Scan()
		if t == token.EOF {
			break
		}
		switch state {
		case afterImport:
			state = inClause
			if t == token.LPAREN {
				state = inImportGroup
			} else if t == token.SEMICOLON {
				state = inCode
			}
			continue
		case inClause:
			if t == token.SEMICOLON {
				state = inCode
			}
			continue
		case inImportGroup:
			if t == token.RPAREN {
				state = afterImportGroup
			}
			continue
		case afterImportGroup:
			state = inCode
			if t == token.SEMICOLON {
				continue
			}
		}
		switch t {
		case token.PACKAGE:
			split++
			state = inClause
			continue
		case token.IMPORT:
			split++
			state = afterImport
			continue
		}
		line := int32(file.Line(pos))
		end := line
		if t == token.STRING && strings.HasPrefix(lit, "`") {
			end += int32(strings.Count(lit, "\n"))
		}
		toks = append(toks, tok{Kind: normalize(t), Start: line, End: end, Split: split})
	}
	if err := errs.Err(); err != nil {
		return nil, err
	}
	return toks, nil
}

// normalize maps every identifier, every string or character literal and
// every number literal to one placeholder each.
func normalize(t token.Token) kind {
	switch t {
	case token.STRING, token.CHAR:
		return kind(token.STRING)
	case token.INT, token.FLOAT, token.IMAG:
		return kind(token.INT)
	}
	return kind(t)
}
