// Refract oracle runner for Go regexp (protocol: oracle/protocol.md).
package main

import (
	"bufio"
	"encoding/json"
	"os"
	"regexp"
	"regexp/syntax"
	"runtime"
	"time"
)

type request struct {
	ID      int64    `json:"id"`
	Pattern string   `json:"pattern"`
	Op      string   `json:"op"`
	Inputs  []string `json:"inputs"`
	Mode    string   `json:"mode"`
}

type response struct {
	ID      int64  `json:"id"`
	OK      bool   `json:"ok"`
	Error   string `json:"error,omitempty"`
	Results []bool  `json:"results"`
	Times   []int64 `json:"times,omitempty"`
	Engine  string `json:"engine,omitempty"`
	Version string `json:"version,omitempty"`
}

func handle(req request) response {
	if req.Op == "version" {
		return response{ID: req.ID, OK: true, Engine: "go-regexp", Version: runtime.Version(), Results: []bool{}}
	}
	if _, err := regexp.Compile(req.Pattern); err != nil {
		return response{ID: req.ID, OK: false, Error: err.Error()}
	}
	src := req.Pattern
	full := req.Op == "full" || (req.Op == "time" && req.Mode == "full")
	if full {
		// Wrap the parsed tree's canonical form, not the raw text: an unterminated \Q would
		// otherwise swallow the wrapper. \A and \z are haystack edges whatever the flags.
		tree, err := syntax.Parse(req.Pattern, syntax.Perl)
		if err != nil {
			return response{ID: req.ID, OK: false, Error: err.Error()}
		}
		src = `\A(?:` + tree.String() + `)\z`
	}
	rx, err := regexp.Compile(src)
	if err != nil {
		return response{ID: req.ID, OK: false, Error: "wrapped: " + err.Error()}
	}
	if req.Op == "time" {
		times := []int64{}
		for _, s := range req.Inputs {
			t0 := time.Now()
			rx.MatchString(s)
			times = append(times, time.Since(t0).Nanoseconds())
		}
		return response{ID: req.ID, OK: true, Results: []bool{}, Times: times}
	}
	results := []bool{}
	if req.Op != "compile" {
		for _, s := range req.Inputs {
			results = append(results, rx.MatchString(s))
		}
	}
	return response{ID: req.ID, OK: true, Results: results}
}

func main() {
	in := bufio.NewScanner(os.Stdin)
	in.Buffer(make([]byte, 1<<20), 1<<26)
	out := bufio.NewWriter(os.Stdout)
	enc := json.NewEncoder(out)
	for in.Scan() {
		line := in.Bytes()
		if len(line) == 0 {
			continue
		}
		var req request
		var resp response
		if err := json.Unmarshal(line, &req); err != nil {
			resp = response{ID: -1, OK: false, Error: "runner: " + err.Error()}
		} else {
			resp = handle(req)
		}
		enc.Encode(resp)
		out.Flush()
	}
}
