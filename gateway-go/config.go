package main

import (
	"os"
	"path/filepath"
	"strings"
)

// Config is the typed gateway configuration sourced from environment variables,
// mirroring the start-up env contract of the original Python service.
type Config struct {
	Host         string
	Port         string
	CoreURL      string
	AllowOrigins []string
	AuthToken    string
	RequireAuth  bool
	JWTSecret    []byte
}

func envStr(key, def string) string {
	if v := strings.TrimSpace(os.Getenv(key)); v != "" {
		return v
	}
	return def
}

func envBool(key string) bool {
	switch strings.TrimSpace(os.Getenv(key)) {
	case "1", "true", "True", "yes":
		return true
	}
	return false
}

func loadConfig() Config {
	host := envStr("AGENT_DEBUG_HOST", "127.0.0.1")
	port := envStr("AGENT_DEBUG_HTTP_PORT", "8002")

	// The Rust core listens on port+1 by default (or AGENT_CORE_URL / AGENT_CORE_PORT).
	coreURL := strings.TrimSpace(os.Getenv("AGENT_CORE_URL"))
	if coreURL == "" {
		corePort := envStr("AGENT_CORE_PORT", "")
		if corePort == "" {
			corePort = bumpPort(port)
		}
		coreURL = "http://127.0.0.1:" + corePort
	}

	origins := envStr("AGENT_DEBUG_ALLOW_ORIGINS",
		"http://127.0.0.1:8030,http://localhost:8030,tauri://localhost,https://tauri.localhost")
	var allow []string
	for _, o := range strings.Split(origins, ",") {
		if t := strings.TrimSpace(o); t != "" {
			allow = append(allow, t)
		}
	}

	dataDir := envStr("AGENT_DEBUG_DATA_DIR", ".")
	secret, _ := os.ReadFile(filepath.Join(dataDir, ".agent_auth_secret"))

	token := strings.TrimSpace(os.Getenv("AGENT_DEBUG_AUTH_TOKEN"))
	return Config{
		Host:         host,
		Port:         port,
		CoreURL:      coreURL,
		AllowOrigins: allow,
		AuthToken:    token,
		RequireAuth:  envBool("AGENT_DEBUG_REQUIRE_AUTH") || token != "",
		JWTSecret:    secret,
	}
}

func bumpPort(p string) string {
	// best-effort port+1
	n := 0
	for _, c := range p {
		if c < '0' || c > '9' {
			return "8003"
		}
		n = n*10 + int(c-'0')
	}
	if n == 0 {
		return "8003"
	}
	return itoa(n + 1)
}

func itoa(n int) string {
	if n == 0 {
		return "0"
	}
	var b [12]byte
	i := len(b)
	for n > 0 {
		i--
		b[i] = byte('0' + n%10)
		n /= 10
	}
	return string(b[i:])
}
