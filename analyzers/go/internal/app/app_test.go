package app

import (
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"io"
	"io/fs"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"testing"

	"github.com/santhosh-tekuri/jsonschema/v6"

	"github.com/rootless-dev/difflens/analyzers/go/internal/analyzer"
	"github.com/rootless-dev/difflens/analyzers/go/internal/checks"
	"github.com/rootless-dev/difflens/analyzers/go/internal/entrypoints"
	"github.com/rootless-dev/difflens/analyzers/go/internal/logging"
	"github.com/rootless-dev/difflens/analyzers/go/internal/mutate"
	"github.com/rootless-dev/difflens/analyzers/go/internal/protocol"
	"github.com/rootless-dev/difflens/analyzers/go/internal/testrun"
)

const schemaPath = "../../../protocol/analysis.schema.json"

var fixtures = []string{"basic", "frameworks", "broken", "testrun", "lint"}

func newApp(stdout, stderr io.Writer) *App {
	log := logging.Discard()
	tests := testrun.New(log)
	return New(stdout, stderr, log, analyzer.New(log, entrypoints.Default()), tests, checks.New(log, checks.DefaultFinder), mutate.New(log, tests))
}

func analyze(t *testing.T, dir string) *protocol.Document {
	t.Helper()
	doc, err := newApp(io.Discard, io.Discard).Analyze(context.Background(), dir)
	if err != nil {
		t.Fatalf("analyze %s: %v", dir, err)
	}
	return doc
}

func runCLI(t *testing.T, args ...string) ([]byte, string, int) {
	t.Helper()
	var stdout, stderr bytes.Buffer
	code := newApp(&stdout, &stderr).Run(args)
	return stdout.Bytes(), stderr.String(), code
}

func function(t *testing.T, doc *protocol.Document, id string) protocol.Function {
	t.Helper()
	for _, f := range doc.Functions {
		if f.ID == id {
			return f
		}
	}
	t.Fatalf("function %s not found", id)
	return protocol.Function{}
}

func callsFrom(doc *protocol.Document, caller string) []protocol.Call {
	var out []protocol.Call
	for _, c := range doc.Calls {
		if c.Caller == caller {
			out = append(out, c)
		}
	}
	return out
}

func requireCall(t *testing.T, doc *protocol.Document, caller, callee, kind, resolution string) {
	t.Helper()
	for _, c := range callsFrom(doc, caller) {
		if c.Callee == callee && c.Relation == protocol.RelationCall {
			if c.Kind != kind || c.Resolution != resolution || c.Evidence != "" {
				t.Errorf("%s -> %s: got %s/%s %q, want %s/%s", caller, callee, c.Kind, c.Resolution, c.Evidence, kind, resolution)
			}
			return
		}
	}
	t.Errorf("missing call %s -> %s; calls from caller: %+v", caller, callee, callsFrom(doc, caller))
}

// relations returns the relations of one kind from caller to callee.
func relations(doc *protocol.Document, caller, callee, relation string) []protocol.Call {
	var out []protocol.Call
	for _, c := range callsFrom(doc, caller) {
		if c.Callee == callee && c.Relation == relation {
			out = append(out, c)
		}
	}
	return out
}

// requireRegistration checks that caller registers callee with evidence.
func requireRegistration(t *testing.T, doc *protocol.Document, caller, callee, evidence string) {
	t.Helper()
	regs := relations(doc, caller, callee, protocol.RelationRegistration)
	for _, c := range regs {
		if c.Evidence == evidence {
			if c.Kind != protocol.KindAsync || c.Resolution != protocol.ResolutionStatic {
				t.Errorf("registration %s -> %s: got %s/%s, want async/static", caller, callee, c.Kind, c.Resolution)
			}
			return
		}
	}
	t.Errorf("missing registration %s -> %s %q; registrations: %+v; calls from caller: %+v",
		caller, callee, evidence, regs, callsFrom(doc, caller))
}

func requireReference(t *testing.T, doc *protocol.Document, caller, callee string) {
	t.Helper()
	refs := relations(doc, caller, callee, protocol.RelationReference)
	if len(refs) == 0 {
		t.Errorf("missing reference %s -> %s; calls from caller: %+v", caller, callee, callsFrom(doc, caller))
		return
	}
	for _, c := range refs {
		if c.Kind != protocol.KindAsync || c.Resolution != protocol.ResolutionStatic || c.Evidence != "" {
			t.Errorf("reference %s -> %s: got %+v", caller, callee, c)
		}
	}
}

func forbidRelation(t *testing.T, doc *protocol.Document, caller, callee, relation string) {
	t.Helper()
	if found := relations(doc, caller, callee, relation); len(found) > 0 {
		t.Errorf("unexpected %s %s -> %s: %+v", relation, caller, callee, found)
	}
}

func requireEntryPoint(t *testing.T, doc *protocol.Document, id, kind, label string) {
	t.Helper()
	f := function(t, doc, id)
	for _, ep := range f.EntryPoints {
		if ep.Kind == kind && ep.Label == label {
			return
		}
	}
	t.Errorf("%s: missing entry point %s %q; has %+v", id, kind, label, f.EntryPoints)
}

func TestBasicModule(t *testing.T) {
	doc := analyze(t, "testdata/basic")
	const svc = "example.com/basic/service."
	const jobs = "example.com/basic/jobs."
	const api = "example.com/basic/api."

	t.Run("call kinds", func(t *testing.T) {
		requireCall(t, doc, svc+"(*Service).Request", svc+"(*Service).cleanup", protocol.KindAlways, protocol.ResolutionStatic) // deferred
		requireCall(t, doc, svc+"(*Service).Request", svc+"(*Service).audit", protocol.KindAlways, protocol.ResolutionStatic)
		requireCall(t, doc, svc+"(*Service).Request", "errors.New", protocol.KindConditional, protocol.ResolutionStatic)              // inside an if
		requireCall(t, doc, svc+"(*Service).Request", svc+"(*Service).validate", protocol.KindConditional, protocol.ResolutionStatic) // after an early return
		requireCall(t, doc, svc+"(*Service).Request", svc+"(*Service).notify", protocol.KindAsync, protocol.ResolutionStatic)         // go statement
		requireCall(t, doc, svc+"(*Service).validate", svc+"(*Service).validate$1", protocol.KindAlways, protocol.ResolutionStatic)
		requireCall(t, doc, jobs+"Loop", jobs+"step", protocol.KindAlways, protocol.ResolutionStatic)
		requireCall(t, doc, jobs+"Loop", jobs+"big", protocol.KindConditional, protocol.ResolutionStatic)
		requireCall(t, doc, jobs+"Consume", jobs+"use", protocol.KindConditional, protocol.ResolutionStatic) // range-over-func body
		requireCall(t, doc, jobs+"Consume", jobs+"Map", protocol.KindAlways, protocol.ResolutionStatic)
		requireCall(t, doc, jobs+"Schedule", jobs+"Schedule$1", protocol.KindAsync, protocol.ResolutionStatic) // time.AfterFunc
		requireCall(t, doc, jobs+"handler$1", jobs+"helper", protocol.KindAlways, protocol.ResolutionStatic)
		requireCall(t, doc, jobs+"init#jobs.go#1", jobs+"helper", protocol.KindAlways, protocol.ResolutionStatic)
	})

	t.Run("dynamic calls", func(t *testing.T) {
		requireCall(t, doc, svc+"(*Service).Request", svc+"(*MemStore).Save", protocol.KindConditional, protocol.ResolutionDynamic)
		requireCall(t, doc, svc+"(*Service).Request", svc+"FileStore.Save", protocol.KindConditional, protocol.ResolutionDynamic)
		requireCall(t, doc, jobs+"Map", jobs+"Consume$1", protocol.KindConditional, protocol.ResolutionDynamic)
		// Writes to an io.Writer holding os.Stdout go to the interface
		// method, not to every io.Writer of the standard library.
		flush := callsFrom(doc, svc+"(*Service).Flush")
		if len(flush) != 1 || flush[0].Callee != "io.Writer.Write" || flush[0].Resolution != protocol.ResolutionDynamic {
			t.Errorf("Flush calls: %+v", flush)
		}
		describe := callsFrom(doc, svc+"Describe")
		if len(describe) != 1 || describe[0].Callee != "builtin.error.Error" {
			t.Errorf("err.Error() must go to the error interface only: %+v", describe)
		}
		// A stub assigned by a test is not a target of production code.
		stamp := callsFrom(doc, svc+"Stamp")
		if len(stamp) != 1 || stamp[0].Callee != svc+"defaultClock" || stamp[0].Resolution != protocol.ResolutionDynamic {
			t.Errorf("Stamp calls: %+v", stamp)
		}
	})

	t.Run("externals", func(t *testing.T) {
		ids := map[string]bool{}
		for _, e := range doc.Externals {
			ids[e.ID] = true
		}
		for _, want := range []string{"fmt.Fprintln", "errors.New", "io.Writer.Write", "net/http.(*ServeMux).HandleFunc", "time.AfterFunc"} {
			if !ids[want] {
				t.Errorf("missing external %s", want)
			}
		}
		if ids["os.(*File).Write"] {
			t.Error("implementation of io.Writer in the standard library listed as external")
		}
	})

	t.Run("complexity and generated", func(t *testing.T) {
		for id, want := range map[string]int{
			svc + "(*Service).Request":  3,
			svc + "(*Service).validate": 2,
			jobs + "Classify":           5,
			jobs + "Loop":               3,
			jobs + "Schedule$1":         1,
		} {
			if got := function(t, doc, id).Complexity; got != want {
				t.Errorf("%s complexity: got %d, want %d", id, got, want)
			}
		}
		if !function(t, doc, svc+"GeneratedHelper").Generated {
			t.Error("GeneratedHelper not flagged as generated")
		}
		if function(t, doc, svc+"NewService").Generated {
			t.Error("NewService flagged as generated")
		}
	})

	t.Run("test code", func(t *testing.T) {
		for _, id := range []string{
			svc + "TestRequest",
			svc + "testingHelper",
			svc + "TestWithClosure",
			svc + "TestWithClosure$1",
			"example.com/basic/service_test.ExampleService_Request",
		} {
			if !function(t, doc, id).Test {
				t.Errorf("%s is in a _test.go file but not flagged as test", id)
			}
		}
		for _, id := range []string{svc + "(*Service).Request", svc + "(*Service).validate$1", svc + "GeneratedHelper", jobs + "Schedule$1"} {
			if function(t, doc, id).Test {
				t.Errorf("%s is production code but flagged as test", id)
			}
		}
	})

	t.Run("names", func(t *testing.T) {
		f := function(t, doc, svc+"(*Service).Request")
		if f.Name != "Service.Request" || f.QualifiedName != "service.Service.Request" || f.Container != "service" || f.File != "service/service.go" {
			t.Errorf("Request: %+v", f)
		}
		function(t, doc, jobs+"(*Box).Get")
	})

	t.Run("entry points", func(t *testing.T) {
		requireEntryPoint(t, doc, "example.com/basic/cmd/basic.main", protocol.EntryMain, "basic")
		requireEntryPoint(t, doc, svc+"TestRequest", protocol.EntryTest, "TestRequest")
		requireEntryPoint(t, doc, svc+"BenchmarkRequest", protocol.EntryTest, "BenchmarkRequest")
		requireEntryPoint(t, doc, "example.com/basic/service_test.ExampleService_Request", protocol.EntryTest, "ExampleService_Request")
		requireEntryPoint(t, doc, api+"(*Handler).Create", protocol.EntryHTTPRoute, "POST /refunds")
		requireEntryPoint(t, doc, api+"Routes$1", protocol.EntryHTTPRoute, "GET /health")
		requireEntryPoint(t, doc, api+"static.ServeHTTP", protocol.EntryHTTPRoute, "/static/")
		requireEntryPoint(t, doc, api+"legacy", protocol.EntryHTTPRoute, "/legacy")
		requireEntryPoint(t, doc, api+"legacy", protocol.EntryHTTPRoute, "/old")
		if eps := function(t, doc, svc+"testingHelper").EntryPoints; len(eps) != 0 {
			t.Errorf("testingHelper has entry points %+v", eps)
		}
	})
}

func TestFrameworks(t *testing.T) {
	doc := analyze(t, "testdata/frameworks")
	const routes = "example.com/frameworks/routes."
	requireEntryPoint(t, doc, routes+"Refunds.List", protocol.EntryHTTPRoute, "GET /refunds")
	requireEntryPoint(t, doc, routes+"Refunds.Create", protocol.EntryHTTPRoute, "POST /refunds")
	requireEntryPoint(t, doc, routes+"cancel", protocol.EntryHTTPRoute, "DELETE /refunds/{id}")
	requireEntryPoint(t, doc, routes+"auth", protocol.EntryHTTPRoute, "GET /items/:id")
	requireEntryPoint(t, doc, routes+"show", protocol.EntryHTTPRoute, "GET /items/:id")
	requireEntryPoint(t, doc, routes+"Gin$1", protocol.EntryHTTPRoute, "POST /v1/items")
	requireEntryPoint(t, doc, "example.com/frameworks/cli.runRefund", protocol.EntryCLICommand, "refund")
	requireEntryPoint(t, doc, "example.com/frameworks/cli.Root$1", protocol.EntryCLICommand, "status")
	// A path that is not constant keeps the route, shown as {dynamic}.
	requireEntryPoint(t, doc, routes+"Refunds.List", protocol.EntryHTTPRoute, "GET {dynamic}")
	// Dependencies are externals, never functions.
	for _, f := range doc.Functions {
		if !strings.HasPrefix(f.ID, "example.com/frameworks/") {
			t.Errorf("function outside the tree: %s", f.ID)
		}
	}
	requireCall(t, doc, routes+"Chi", "github.com/go-chi/chi/v5.Router.Post", protocol.KindAlways, protocol.ResolutionDynamic)
}

func TestRoutesFoldConstantsAndPrefixes(t *testing.T) {
	doc := analyze(t, "testdata/frameworks")
	const routes = "example.com/frameworks/routes."
	for id, label := range map[string]string{
		routes + "Items.Show":    "GET /api/items/{id}", // constants folded, through a router interface of the tree
		routes + "Items.List":    "GET /api/items",      // chi Route prefix
		routes + "Items.Create":  "POST /api/items",     // chi With keeps the prefix
		routes + "Items.Admin":   "GET /api/admin",      // chi Group inside Route
		routes + "Items.Vars":    "GET /debug/vars",     // chi Mount of a router of the same function
		routes + "Items.ByID":    "GET {dynamic}/{id}",  // a part that is not constant
		routes + "serveItem":     "GET /item",           // http.HandlerFunc(f)
		routes + "ginList":       "GET /v2/items",       // gin Group
		routes + "ginCreateUser": "POST /v2/admin/users",
	} {
		requireEntryPoint(t, doc, id, protocol.EntryHTTPRoute, label)
	}
	// The handler a wrapper wraps is the route's handler.
	requireEntryPoint(t, doc, routes+"Items.Create", protocol.EntryHTTPRoute, "POST /items")
	if eps := function(t, doc, routes+"withLogging").EntryPoints; len(eps) != 0 {
		t.Errorf("a wrapper is not an entry point: %+v", eps)
	}
}

func TestRegistrations(t *testing.T) {
	doc := analyze(t, "testdata/frameworks")
	const routes = "example.com/frameworks/routes."
	chi, api, admin := routes+"ChiFlow", routes+"ChiFlow$1", routes+"ChiFlow$1$1"

	t.Run("routes", func(t *testing.T) {
		requireRegistration(t, doc, routes+"Items.Mount", routes+"Items.Show", "route GET /api/items/{id}")
		requireRegistration(t, doc, api, routes+"Items.List", "route GET /api/items")
		requireRegistration(t, doc, api, routes+"Items.Create", "route POST /api/items")
		requireRegistration(t, doc, admin, routes+"Items.Admin", "route GET /api/admin")
		requireRegistration(t, doc, chi, routes+"Items.Vars", "route GET /debug/vars")
		requireRegistration(t, doc, chi, routes+"Items.ByID", "route GET {dynamic}/{id}")
		requireRegistration(t, doc, chi, routes+"notFound", "not found handler")
		requireRegistration(t, doc, routes+"NetHTTP", routes+"serveItem", "route GET /item")
		requireRegistration(t, doc, routes+"NetHTTP", routes+"Items.Create", "route POST /items")
		requireRegistration(t, doc, routes+"GinFlow", routes+"ginList", "route GET /v2/items")
		requireRegistration(t, doc, routes+"GinFlow", routes+"ginCreateUser", "route POST /v2/admin/users")
	})

	t.Run("groups", func(t *testing.T) {
		requireRegistration(t, doc, chi, api, "route group /api")
		requireRegistration(t, doc, api, admin, "route group /api")
	})

	t.Run("middlewares", func(t *testing.T) {
		requireRegistration(t, doc, chi, routes+"logRequests", "middleware")                // a function
		requireRegistration(t, doc, chi, routes+"Auth.Check", "middleware")                 // a method value
		requireRegistration(t, doc, chi, routes+"RequestID$1", "middleware")                // the closure a constructor returns
		requireRegistration(t, doc, chi, routes+"Shared", "middleware")                     // a constructor whose result is not known
		requireRegistration(t, doc, api, routes+"Auth.Check", "middleware")                 // chi With
		requireRegistration(t, doc, admin, routes+"logRequests", "middleware")              // inside a group
		requireRegistration(t, doc, routes+"NetHTTP", routes+"withLogging$1", "middleware") // mw(next)
		requireRegistration(t, doc, routes+"GinFlow", routes+"ginLogger", "middleware")
		requireRegistration(t, doc, routes+"GinFlow", routes+"ginAuth", "middleware") // gin Group handlers
		forbidRelation(t, doc, chi, routes+"RequestID", protocol.RelationRegistration)
		// The constructor is still called.
		requireCall(t, doc, chi, routes+"RequestID", protocol.KindAlways, protocol.ResolutionStatic)
	})

	t.Run("commands", func(t *testing.T) {
		requireRegistration(t, doc, "example.com/frameworks/cli.Root", "example.com/frameworks/cli.runRefund", "command refund")
		requireRegistration(t, doc, "example.com/frameworks/cli.Root", "example.com/frameworks/cli.Root$1", "command status")
	})

	t.Run("no reference where there is a registration", func(t *testing.T) {
		for _, pair := range [][2]string{
			{chi, routes + "logRequests"},
			{chi, api},
			{api, routes + "Items.List"},
			{routes + "NetHTTP", routes + "serveItem"},
			{routes + "NetHTTP", routes + "Items.Create"},
			{"example.com/frameworks/cli.Root", "example.com/frameworks/cli.runRefund"},
		} {
			forbidRelation(t, doc, pair[0], pair[1], protocol.RelationReference)
		}
	})
}

func TestRegistrationsInBasicModule(t *testing.T) {
	doc := analyze(t, "testdata/basic")
	const api = "example.com/basic/api."
	requireRegistration(t, doc, api+"Routes", api+"legacy", "route /legacy")
	requireRegistration(t, doc, api+"Routes", api+"legacy", "route /old")
	requireRegistration(t, doc, api+"Routes", api+"(*Handler).Create", "route POST /refunds")
	requireRegistration(t, doc, api+"Routes", api+"Routes$1", "route GET /health")
	requireRegistration(t, doc, api+"Routes", api+"static.ServeHTTP", "route /static/")
	forbidRelation(t, doc, api+"Routes", api+"Routes$1", protocol.RelationReference)
}

func TestReferences(t *testing.T) {
	doc := analyze(t, "testdata/frameworks")
	const routes = "example.com/frameworks/routes."
	requireReference(t, doc, routes+"References", routes+"task")       // a callback given to an unknown function
	requireReference(t, doc, routes+"References", routes+"Items.Show") // stored in a struct field
	requireReference(t, doc, routes+"RequestID", routes+"RequestID$1") // returned
	// A method value called on the spot is a call only.
	requireCall(t, doc, routes+"References", routes+"Items.List", protocol.KindAlways, protocol.ResolutionStatic)
	forbidRelation(t, doc, routes+"References", routes+"Items.List", protocol.RelationReference)
	// The callback is called by the function it is given to.
	requireCall(t, doc, routes+"run", routes+"task", protocol.KindAlways, protocol.ResolutionDynamic)

	for _, c := range doc.Calls {
		if c.Caller == c.Callee && c.Relation != protocol.RelationCall {
			t.Errorf("self relation: %+v", c)
		}
	}
}

func TestRelationsAreDeduplicated(t *testing.T) {
	doc := &protocol.Document{Calls: []protocol.Call{
		{Caller: "a", Callee: "b", Line: 3, Kind: protocol.KindAsync, Resolution: protocol.ResolutionStatic, Relation: protocol.RelationRegistration, Evidence: "route POST /x"},
		{Caller: "a", Callee: "b", Line: 3, Kind: protocol.KindAsync, Resolution: protocol.ResolutionStatic, Relation: protocol.RelationRegistration, Evidence: "route GET /x"},
		{Caller: "a", Callee: "b", Line: 3, Kind: protocol.KindAsync, Resolution: protocol.ResolutionStatic, Relation: protocol.RelationRegistration, Evidence: "route GET /x"},
		{Caller: "a", Callee: "b", Line: 3, Kind: protocol.KindAsync, Resolution: protocol.ResolutionStatic, Relation: protocol.RelationReference},
		{Caller: "a", Callee: "b", Line: 3, Kind: protocol.KindAsync, Resolution: protocol.ResolutionStatic, Relation: protocol.RelationReference},
		{Caller: "a", Callee: "b", Line: 3, Kind: protocol.KindAlways, Resolution: protocol.ResolutionStatic, Relation: protocol.RelationCall},
	}}
	doc.Normalize()
	if len(doc.Calls) != 3 {
		t.Fatalf("got %d relations, want 3: %+v", len(doc.Calls), doc.Calls)
	}
	for _, c := range doc.Calls {
		if c.Relation == protocol.RelationRegistration && c.Evidence != "route GET /x; route POST /x" {
			t.Errorf("evidence not joined: %q", c.Evidence)
		}
	}
}

func TestBrokenPackageIsADiagnostic(t *testing.T) {
	out, stderr, code := runCLI(t, "analyze", "--dir", "testdata/broken")
	if code != 0 {
		t.Fatalf("exit %d: %s", code, stderr)
	}
	doc := decode(t, out)
	found := false
	for _, d := range doc.Diagnostics {
		if d.Level == protocol.LevelError && d.File == "bad/bad.go" && d.Line == 6 {
			found = true
		}
		if strings.HasPrefix(d.File, "/") || strings.Contains(d.File, ":") {
			t.Errorf("diagnostic file is not a clean relative path: %+v", d)
		}
	}
	if !found {
		t.Errorf("no error diagnostic at bad/bad.go:6: %+v", doc.Diagnostics)
	}
	function(t, doc, "example.com/broken/bad.helper")
	requireCall(t, doc, "example.com/broken/good.Run", "example.com/broken/bad.Do", protocol.KindAlways, protocol.ResolutionStatic)
}

func decode(t *testing.T, out []byte) *protocol.Document {
	t.Helper()
	var doc protocol.Document
	if err := json.Unmarshal(out, &doc); err != nil {
		t.Fatalf("decode output: %v", err)
	}
	return &doc
}

func TestIDsAreStableAcrossEdits(t *testing.T) {
	dir := t.TempDir()
	copyTree(t, "testdata/basic", dir)
	before := analyze(t, dir)

	file := filepath.Join(dir, "service", "service.go")
	src, err := os.ReadFile(file)
	if err != nil {
		t.Fatal(err)
	}
	edited := strings.Replace(string(src), "// Store has two implementations",
		"func unrelated() {\n\tf := func() {}\n\tg := func() {}\n\tf()\n\tg()\n}\n\n// Store has two implementations", 1)
	edited = strings.Replace(edited, "\ts.audit(key)\n",
		"\ts.audit(key)\n\tif len(key) > 10 {\n\t\tfunc() { s.audit(key[:10]) }()\n\t}\n", 1)
	if edited == string(src) {
		t.Fatal("edit did not apply")
	}
	if err := os.WriteFile(file, []byte(edited), 0o644); err != nil {
		t.Fatal(err)
	}
	after := analyze(t, dir)

	ids := map[string]protocol.Function{}
	for _, f := range after.Functions {
		ids[f.ID] = f
	}
	for _, f := range before.Functions {
		if _, ok := ids[f.ID]; !ok {
			t.Errorf("id %s disappeared after an unrelated edit", f.ID)
		}
	}
	if ids["example.com/basic/service.(*Service).validate"].StartLine == function(t, before, "example.com/basic/service.(*Service).validate").StartLine {
		t.Error("the edit did not move validate; the test proves nothing")
	}
	for _, id := range []string{"example.com/basic/service.unrelated$2", "example.com/basic/service.(*Service).Request$1"} {
		if _, ok := ids[id]; !ok {
			t.Errorf("new closure %s missing", id)
		}
	}
}

func TestOutputMatchesSchemaAndIsDeterministic(t *testing.T) {
	compiler := jsonschema.NewCompiler()
	schema, err := compiler.Compile(schemaPath)
	if err != nil {
		t.Fatal(err)
	}
	for _, fixture := range fixtures {
		t.Run(fixture, func(t *testing.T) {
			out, stderr, code := runCLI(t, "analyze", "--dir", filepath.Join("testdata", fixture))
			if code != 0 {
				t.Fatalf("exit %d: %s", code, stderr)
			}
			inst, err := jsonschema.UnmarshalJSON(bytes.NewReader(out))
			if err != nil {
				t.Fatal(err)
			}
			if err := schema.Validate(inst); err != nil {
				t.Errorf("output does not match the schema: %v", err)
			}
			again, _, _ := runCLI(t, "analyze", "--dir", filepath.Join("testdata", fixture))
			if !bytes.Equal(out, again) {
				t.Error("two runs produced different output")
			}
		})
	}
}

func TestSourceTreeIsNotModified(t *testing.T) {
	dir := t.TempDir()
	copyTree(t, "testdata", dir)
	before := hashTree(t, dir)
	for _, fixture := range fixtures {
		if _, stderr, code := runCLI(t, "analyze", "--dir", filepath.Join(dir, fixture)); code != 0 {
			t.Fatalf("%s: exit %d: %s", fixture, code, stderr)
		}
	}
	if after := hashTree(t, dir); after != before {
		t.Error("the analysed tree changed")
	}
}

func TestWorkspaceAndMultiModuleTrees(t *testing.T) {
	files := map[string]string{
		"a/go.mod": "module example.com/a\n\ngo 1.24\n",
		"a/a.go":   "package a\n\nfunc A() {}\n",
		"b/go.mod": "module example.com/b\n\ngo 1.24\n\nrequire example.com/a v0.0.0\n\nreplace example.com/a => ../a\n",
		"b/b.go":   "package b\n\nimport \"example.com/a\"\n\nfunc B() { a.A() }\n",
	}
	for _, workspace := range []bool{false, true} {
		dir := t.TempDir()
		for name, content := range files {
			writeFile(t, filepath.Join(dir, name), content)
		}
		if workspace {
			writeFile(t, filepath.Join(dir, "go.work"), "go 1.24\n\nuse (\n\t./a\n\t./b\n)\n")
		}
		before := hashTree(t, dir)
		doc := analyze(t, dir)
		function(t, doc, "example.com/a.A")
		requireCall(t, doc, "example.com/b.B", "example.com/a.A", protocol.KindAlways, protocol.ResolutionStatic)
		for _, e := range doc.Externals {
			if e.ID == "example.com/a.A" {
				t.Errorf("workspace=%v: a function of the tree is also an external", workspace)
			}
		}
		if hashTree(t, dir) != before {
			t.Errorf("workspace=%v: the tree changed", workspace)
		}
	}
}

func writeFile(t *testing.T, path, content string) {
	t.Helper()
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, []byte(content), 0o644); err != nil {
		t.Fatal(err)
	}
}

func TestVersion(t *testing.T) {
	out, _, code := runCLI(t, "--version")
	if code != 0 || string(out) != Name+" "+Version+"\n" {
		t.Errorf("got %q, exit %d", out, code)
	}
}

func TestFailures(t *testing.T) {
	if _, _, code := runCLI(t, "analyze", "--dir", t.TempDir()); code == 0 {
		t.Error("a tree without go.mod must fail")
	}
	if _, _, code := runCLI(t, "analyze", "--dir", "testdata/basic", "--timeout", "1ns"); code == 0 {
		t.Error("an expired timeout must fail")
	}
	if _, _, code := runCLI(t, "analyze"); code != exitUsage {
		t.Error("missing --dir must be a usage error")
	}
}

func copyTree(t *testing.T, src, dst string) {
	t.Helper()
	err := filepath.WalkDir(src, func(path string, d fs.DirEntry, err error) error {
		if err != nil {
			return err
		}
		rel, err := filepath.Rel(src, path)
		if err != nil {
			return err
		}
		target := filepath.Join(dst, rel)
		if d.IsDir() {
			return os.MkdirAll(target, 0o755)
		}
		data, err := os.ReadFile(path)
		if err != nil {
			return err
		}
		return os.WriteFile(target, data, 0o644)
	})
	if err != nil {
		t.Fatal(err)
	}
}

// hashTree hashes every path, mode and file content under dir.
func hashTree(t *testing.T, dir string) string {
	t.Helper()
	var lines []string
	err := filepath.WalkDir(dir, func(path string, d fs.DirEntry, err error) error {
		if err != nil {
			return err
		}
		info, err := d.Info()
		if err != nil {
			return err
		}
		entry := path + " " + info.Mode().String()
		if !d.IsDir() {
			data, err := os.ReadFile(path)
			if err != nil {
				return err
			}
			sum := sha256.Sum256(data)
			entry += " " + hex.EncodeToString(sum[:])
		}
		lines = append(lines, entry)
		return nil
	})
	if err != nil {
		t.Fatal(err)
	}
	slices.Sort(lines)
	sum := sha256.Sum256([]byte(strings.Join(lines, "\n")))
	return hex.EncodeToString(sum[:])
}
