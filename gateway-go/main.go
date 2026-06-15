// Command agent-debug-gateway is the Go edge gateway for the Agent Debug
// backend. It terminates client HTTP / WebSocket / SSE, enforces CORS and
// bearer/JWT auth, and reverse-proxies REST to the Rust core service. WebSocket
// subscribers are driven from the core's SSE stream (replay + live fan-out).
package main

import (
	"log"
	"net/http"
	"net/http/httputil"
	"net/url"
	"strings"
)

type Server struct {
	cfg   Config
	proxy *httputil.ReverseProxy
}

var openPaths = []string{
	"/health",
	"/api/agent-debug/auth/login",
	"/api/agent-debug/auth/register",
}

func main() {
	cfg := loadConfig()
	core, err := url.Parse(cfg.CoreURL)
	if err != nil {
		log.Fatalf("invalid AGENT_CORE_URL: %v", err)
	}
	proxy := httputil.NewSingleHostReverseProxy(core)

	s := &Server{cfg: cfg, proxy: proxy}

	addr := cfg.Host + ":" + cfg.Port
	log.Printf("agent-debug-gateway (Go edge) listening on http://%s -> core %s", addr, cfg.CoreURL)
	if err := http.ListenAndServe(addr, http.HandlerFunc(s.handle)); err != nil {
		log.Fatal(err)
	}
}

func (s *Server) handle(w http.ResponseWriter, r *http.Request) {
	s.applyCORS(w, r)
	if r.Method == http.MethodOptions {
		w.WriteHeader(http.StatusNoContent)
		return
	}

	// WebSocket endpoint handles its own auth (token gate in the subscribe frame).
	if r.URL.Path == "/ws/agent-debug" {
		s.handleWS(w, r)
		return
	}

	if !s.authorized(r) {
		writeError(w, http.StatusUnauthorized, "AUTH_MISSING", "Authentication required")
		return
	}

	s.proxy.ServeHTTP(w, r)
}

func (s *Server) authorized(r *http.Request) bool {
	// Exact match: prefix matching would also open e.g. /auth/login-anything.
	for _, p := range openPaths {
		if r.URL.Path == p {
			return true
		}
	}
	bearer := ""
	if h := r.Header.Get("Authorization"); strings.HasPrefix(h, "Bearer ") {
		bearer = strings.TrimSpace(strings.TrimPrefix(h, "Bearer "))
	}
	if bearer != "" {
		// A present-but-invalid bearer is always rejected.
		return s.cfg.credentialOK(bearer)
	}
	// Missing bearer: permit only when enforcement is off (local-dev default).
	return !s.cfg.RequireAuth
}

func (s *Server) applyCORS(w http.ResponseWriter, r *http.Request) {
	origin := r.Header.Get("Origin")
	if origin == "" {
		return
	}
	for _, allowed := range s.cfg.AllowOrigins {
		if allowed == origin {
			h := w.Header()
			h.Set("Access-Control-Allow-Origin", origin)
			h.Set("Vary", "Origin")
			h.Set("Access-Control-Allow-Credentials", "true")
			h.Set("Access-Control-Allow-Methods", "GET, POST, PUT, PATCH, DELETE, OPTIONS")
			h.Set("Access-Control-Allow-Headers", "Content-Type, Authorization")
			return
		}
	}
}

func writeError(w http.ResponseWriter, status int, code, msg string) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(status)
	_, _ = w.Write([]byte(`{"error":{"code":"` + code + `","message":"` + msg + `"}}`))
}
