package main

import (
	"bufio"
	"context"
	"encoding/json"
	"net/http"
	"net/url"
	"strconv"
	"strings"
	"sync"
	"time"

	"github.com/gorilla/websocket"
)

var upgrader = websocket.Upgrader{
	ReadBufferSize:  4096,
	WriteBufferSize: 4096,
	// Origin is enforced at the auth layer; allow the handshake here.
	CheckOrigin: func(r *http.Request) bool { return true },
}

// wsConn serializes writes (gorilla allows only one concurrent writer).
type wsConn struct {
	conn *websocket.Conn
	mu   sync.Mutex
}

func (w *wsConn) writeText(b []byte) error {
	w.mu.Lock()
	defer w.mu.Unlock()
	return w.conn.WriteMessage(websocket.TextMessage, b)
}

type subscribeMsg struct {
	Action    string   `json:"action"`
	SessionID string   `json:"sessionId"`
	FromSeq   *int64   `json:"fromSeq"`
	Channels  []string `json:"channels"`
	Token     string   `json:"token"`
}

func (s *Server) handleWS(w http.ResponseWriter, r *http.Request) {
	conn, err := upgrader.Upgrade(w, r, nil)
	if err != nil {
		return
	}
	wc := &wsConn{conn: conn}
	defer conn.Close()

	var cancelPrev context.CancelFunc
	defer func() {
		if cancelPrev != nil {
			cancelPrev()
		}
	}()

	for {
		_, raw, err := conn.ReadMessage()
		if err != nil {
			return
		}
		var msg subscribeMsg
		if json.Unmarshal(raw, &msg) != nil || msg.Action != "subscribe" || msg.SessionID == "" {
			continue
		}
		// Token gate (unified: static token OR account JWT).
		if s.cfg.AuthToken != "" && !s.cfg.credentialOK(strings.TrimSpace(msg.Token)) {
			_ = conn.WriteControl(websocket.CloseMessage,
				websocket.FormatCloseMessage(websocket.ClosePolicyViolation, "unauthorized"),
				time.Now().Add(time.Second))
			return
		}

		if cancelPrev != nil {
			cancelPrev()
		}
		ctx, cancel := context.WithCancel(r.Context())
		cancelPrev = cancel
		go s.streamSession(ctx, wc, msg)
	}
}

// streamSession replays fromSeq backlog (with gap detection) then forwards live
// SSE events from the Rust core, filtered by channel.
func (s *Server) streamSession(ctx context.Context, wc *wsConn, msg subscribeMsg) {
	fromSeq := int64(0)
	if msg.FromSeq != nil {
		fromSeq = *msg.FromSeq
	}
	channels := toSet(msg.Channels)

	// 1) Replay backlog + gap detection via the JSON replay endpoint.
	replayURL := s.cfg.CoreURL + "/api/agent-debug/replay/" + url.PathEscape(msg.SessionID) +
		"/since?fromSeq=" + strconv.FormatInt(fromSeq, 10)
	req, _ := http.NewRequestWithContext(ctx, http.MethodGet, replayURL, nil)
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		return
	}
	var replay struct {
		Events    []json.RawMessage `json:"events"`
		Gap       bool              `json:"gap"`
		LatestSeq int64             `json:"latestSeq"`
	}
	_ = json.NewDecoder(resp.Body).Decode(&replay)
	resp.Body.Close()

	if msg.FromSeq != nil && replay.Gap {
		gap, _ := json.Marshal(map[string]any{
			"type":            "ws.replay.gap",
			"sessionId":       msg.SessionID,
			"requestedFromSeq": fromSeq,
			"latestSeq":       replay.LatestSeq,
			"code":            "WS_REPLAY_GAP_TOO_LARGE",
			"message":         "Requested fromSeq is older than the buffered window. Re-fetch GET /design-snapshot and resubscribe.",
		})
		_ = wc.writeText(gap)
		return
	}

	for _, ev := range replay.Events {
		if channelAllowed(ev, channels) {
			_ = wc.writeText(ev)
		}
	}
	if msg.FromSeq != nil {
		sub, _ := json.Marshal(map[string]any{
			"type":      "ws.subscribed",
			"sessionId": msg.SessionID,
			"latestSeq": replay.LatestSeq,
		})
		_ = wc.writeText(sub)
	}

	// 2) Live forwarding via SSE from the core (resume at latestSeq).
	sseURL := s.cfg.CoreURL + "/api/agent-debug/sessions/" + url.PathEscape(msg.SessionID) +
		"/events/stream?fromSeq=" + strconv.FormatInt(replay.LatestSeq, 10)
	sreq, _ := http.NewRequestWithContext(ctx, http.MethodGet, sseURL, nil)
	sreq.Header.Set("Accept", "text/event-stream")
	sresp, err := http.DefaultClient.Do(sreq)
	if err != nil {
		return
	}
	defer sresp.Body.Close()

	scanner := bufio.NewScanner(sresp.Body)
	scanner.Buffer(make([]byte, 0, 64*1024), 4*1024*1024)
	for scanner.Scan() {
		select {
		case <-ctx.Done():
			return
		default:
		}
		line := scanner.Text()
		data, ok := strings.CutPrefix(line, "data:")
		if !ok {
			continue
		}
		payload := []byte(strings.TrimSpace(data))
		if len(payload) == 0 {
			continue
		}
		if channelAllowed(payload, channels) {
			if wc.writeText(payload) != nil {
				return
			}
		}
	}
}

func toSet(items []string) map[string]bool {
	if len(items) == 0 {
		return nil
	}
	m := make(map[string]bool, len(items))
	for _, it := range items {
		m[it] = true
	}
	return m
}

func channelAllowed(eventJSON []byte, channels map[string]bool) bool {
	if channels == nil {
		return true
	}
	var probe struct {
		Channel string `json:"channel"`
	}
	if json.Unmarshal(eventJSON, &probe) != nil {
		return true
	}
	return channels[probe.Channel]
}
