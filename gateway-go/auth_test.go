package main

import (
	"crypto/hmac"
	"crypto/sha256"
	"encoding/base64"
	"encoding/json"
	"testing"
	"time"
)

func mintJWT(secret []byte, exp int64) string {
	header, _ := json.Marshal(map[string]string{"alg": "HS256", "typ": "JWT"})
	payload, _ := json.Marshal(map[string]int64{"iat": time.Now().Unix(), "exp": exp})
	h := base64.RawURLEncoding.EncodeToString(header)
	p := base64.RawURLEncoding.EncodeToString(payload)
	mac := hmac.New(sha256.New, secret)
	mac.Write([]byte(h + "." + p))
	sig := base64.RawURLEncoding.EncodeToString(mac.Sum(nil))
	return h + "." + p + "." + sig
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
}
