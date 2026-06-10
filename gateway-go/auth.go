package main

import (
	"crypto/hmac"
	"crypto/sha256"
	"crypto/subtle"
	"encoding/base64"
	"encoding/json"
	"strings"
	"time"
)

// verifyJWT validates a self-issued HS256 token (matching the Rust AuthService).
// Returns true if signature + expiry are valid. Unifying this with the REST
// auth means WebSocket subscribers can authenticate with a JWT too — a fix for
// the Python service where WS only accepted the static token.
func verifyJWT(token string, secret []byte) bool {
	if len(secret) == 0 {
		return false
	}
	parts := strings.Split(token, ".")
	if len(parts) != 3 {
		return false
	}
	signingInput := parts[0] + "." + parts[1]
	mac := hmac.New(sha256.New, secret)
	mac.Write([]byte(signingInput))
	expected := mac.Sum(nil)

	got, err := base64.RawURLEncoding.DecodeString(parts[2])
	if err != nil {
		return false
	}
	if subtle.ConstantTimeCompare(expected, got) != 1 {
		return false
	}

	payloadRaw, err := base64.RawURLEncoding.DecodeString(parts[1])
	if err != nil {
		return false
	}
	var claims struct {
		Exp int64 `json:"exp"`
	}
	if json.Unmarshal(payloadRaw, &claims) != nil {
		return false
	}
	return time.Now().Unix() <= claims.Exp
}

// credentialOK returns true if the bearer is the static service token or a
// valid account JWT.
func (c Config) credentialOK(bearer string) bool {
	if bearer == "" {
		return false
	}
	if c.AuthToken != "" && subtle.ConstantTimeCompare([]byte(bearer), []byte(c.AuthToken)) == 1 {
		return true
	}
	return verifyJWT(bearer, c.JWTSecret)
}
