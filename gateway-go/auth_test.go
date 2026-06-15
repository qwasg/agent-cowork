package main

import (
	"crypto/hmac"
	"crypto/sha256"
	"encoding/base64"
	"encoding/json"
	"net/http/httptest"
	"testing"
	"time"
)

func mintJWTWithHeader(secret []byte, headerMap map[string]string, payloadMap map[string]int64) string {
	header, _ := json.Marshal(headerMap)
	payload, _ := json.Marshal(payloadMap)
	h := base64.RawURLEncoding.EncodeToString(header)
	p := base64.RawURLEncoding.EncodeToString(payload)
	mac := hmac.New(sha256.New, secret)
	mac.Write([]byte(h + "." + p))
	sig := base64.RawURLEncoding.EncodeToString(mac.Sum(nil))
	return h + "." + p + "." + sig
}

func mintJWT(secret []byte, exp int64) string {
	return mintJWTWithHeader(
		secret,
		map[string]string{"alg": "HS256", "typ": "JWT"},
		map[string]int64{"iat": time.Now().Unix(), "exp": exp},
	)
}

func TestVerifyJWT(t *testing.T) {
	secret := []byte("super-secret-key-for-tests-1234567890")

	valid := mintJWT(secret, time.Now().Add(time.Hour).Unix())
	if !verifyJWT(valid, secret) {
		t.Fatal("expected valid token to verify")
	}

	expired := mintJWT(secret, time.Now().Add(-time.Hour).Unix())
	if verifyJWT(expired, secret) {
		t.Fatal("expected expired token to fail")
	}

	if verifyJWT(valid, []byte("wrong-secret")) {
		t.Fatal("expected wrong secret to fail")
	}

	if verifyJWT("not.a.jwt", secret) {
		t.Fatal("expected malformed token to fail")
	}
}

func TestCredentialOK(t *testing.T) {
	cfg := Config{AuthToken: "static-token", JWTSecret: []byte("secret-key-secret-key-secret-key")}
	if !cfg.credentialOK("static-token") {
		t.Fatal("static token should pass")
	}
	if cfg.credentialOK("nope") {
		t.Fatal("wrong credential should fail")
	}
	jwt := mintJWT(cfg.JWTSecret, time.Now().Add(time.Hour).Unix())
	if !cfg.credentialOK(jwt) {
		t.Fatal("valid jwt should pass")
	}
	if cfg.credentialOK("") {
		t.Fatal("empty bearer should fail")
	}
}

func TestVerifyJWTRejectsAlgConfusionAndMissingExp(t *testing.T) {
	secret := []byte("super-secret-key-for-tests-1234567890")

	// alg=none (still HS256-signed, but header must declare HS256).
	none := mintJWTWithHeader(
		secret,
		map[string]string{"alg": "none", "typ": "JWT"},
		map[string]int64{"exp": time.Now().Add(time.Hour).Unix()},
	)
	if verifyJWT(none, secret) {
		t.Fatal("expected alg!=HS256 token to fail")
	}

	// Missing exp claim → rejected.
	noExp := mintJWTWithHeader(
		secret,
		map[string]string{"alg": "HS256", "typ": "JWT"},
		map[string]int64{"iat": time.Now().Unix()},
	)
	if verifyJWT(noExp, secret) {
		t.Fatal("expected token without exp to fail")
	}

	// Empty secret never verifies.
	if verifyJWT(mintJWT(secret, time.Now().Add(time.Hour).Unix()), nil) {
		t.Fatal("expected empty secret to fail")
	}
}

func TestAuthorizedOpenPathsAreExactMatch(t *testing.T) {
	s := &Server{cfg: Config{RequireAuth: true, JWTSecret: []byte("secret-key-secret-key-secret-key")}}

	for _, path := range openPaths {
		r := httptest.NewRequest("POST", path, nil)
		if !s.authorized(r) {
			t.Fatalf("open path %s should be authorized without bearer", path)
		}
	}

	// Prefix lookalikes must NOT bypass auth.
	for _, path := range []string{
		"/api/agent-debug/auth/login-bypass",
		"/api/agent-debug/auth/register/extra",
		"/healthz",
	} {
		r := httptest.NewRequest("POST", path, nil)
		if s.authorized(r) {
			t.Fatalf("lookalike path %s must require auth", path)
		}
	}

	// Enforced mode: missing bearer on a protected path is rejected,
	// valid JWT passes.
	r := httptest.NewRequest("GET", "/api/agent-debug/sessions", nil)
	if s.authorized(r) {
		t.Fatal("missing bearer must fail when RequireAuth is on")
	}
	r.Header.Set("Authorization", "Bearer "+mintJWT(s.cfg.JWTSecret, time.Now().Add(time.Hour).Unix()))
	if !s.authorized(r) {
		t.Fatal("valid jwt must pass on protected path")
	}

	// Relaxed mode (local dev): missing bearer is tolerated, but a present
	// invalid bearer is still rejected.
	relaxed := &Server{cfg: Config{RequireAuth: false, JWTSecret: []byte("secret-key-secret-key-secret-key")}}
	r = httptest.NewRequest("GET", "/api/agent-debug/sessions", nil)
	if !relaxed.authorized(r) {
		t.Fatal("missing bearer should pass in relaxed mode")
	}
	r.Header.Set("Authorization", "Bearer garbage")
	if relaxed.authorized(r) {
		t.Fatal("invalid bearer must always be rejected")
	}
}
