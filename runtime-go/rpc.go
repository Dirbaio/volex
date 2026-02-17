// Package volex provides runtime encoding/decoding support.
package volex

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"io"
	"net/http"
	"sync"
)

// ============================================================================
// RPC Errors
// ============================================================================

var (
	ErrStreamClosed = errors.New("stream closed")
)

// RpcError represents an RPC error with a code and message.
type RpcError struct {
	Code    uint32
	Message string
}

func (e *RpcError) Error() string {
	return e.Message
}

// Error codes
const (
	ErrCodeUnknownMethod uint32 = 1
	ErrCodeDecodeError   uint32 = 2
	ErrCodeHandlerError  uint32 = 3
)

// ============================================================================
// RPC Message Types
// ============================================================================

const (
	// Server -> Client message types (0x00-0x7F)
	rpcTypeResponse   byte = 0x00
	rpcTypeStreamItem byte = 0x01
	rpcTypeStreamEnd  byte = 0x02
	rpcTypeError      byte = 0x03

	// Client -> Server message types (0x80-0xFF)
	rpcTypeRequest byte = 0x80
	rpcTypeCancel  byte = 0x81
)

// ============================================================================
// High-level Transport Interfaces
// ============================================================================

// ClientTransport is the high-level client transport interface.
// Generated client code uses this interface.
type ClientTransport interface {
	// CallUnary makes a unary RPC call.
	CallUnary(ctx context.Context, methodIndex uint32, payload []byte) ([]byte, error)
	// CallStream makes a streaming RPC call.
	CallStream(ctx context.Context, methodIndex uint32, payload []byte) (*StreamReceiver, error)
}

// ServerHandler handles incoming RPC calls.
// Generated server code implements this interface.
type ServerHandler interface {
	// HandleCall handles a single incoming RPC call.
	HandleCall(call ServerCall)
}

// ServerCall represents a single incoming RPC request.
type ServerCall interface {
	// MethodIndex returns the method index.
	MethodIndex() uint32
	// Payload returns the request payload.
	Payload() []byte
	// Context returns the context for this call. The context is canceled when the client cancels the request.
	Context() context.Context
	// SendResponse sends a unary response.
	SendResponse(payload []byte) error
	// SendStreamItem sends a stream item.
	SendStreamItem(payload []byte) error
	// SendStreamEnd signals the end of a stream.
	SendStreamEnd() error
	// SendError sends an error response.
	SendError(code uint32, message string) error
}

// StreamReceiver is used to receive streaming responses.
type StreamReceiver struct {
	streamChan <-chan streamResult
	cancelFunc func()
	ctx        context.Context
}

// Recv receives the next stream item. Returns nil, ErrStreamClosed when the stream ends.
func (s *StreamReceiver) Recv() ([]byte, error) {
	select {
	case <-s.ctx.Done():
		s.Cancel()
		return nil, s.ctx.Err()
	case res := <-s.streamChan:
		if res.err != nil {
			return nil, res.err
		}
		return res.data, nil
	}
}

// Cancel cancels the stream.
func (s *StreamReceiver) Cancel() {
	if s.cancelFunc != nil {
		s.cancelFunc()
		s.cancelFunc = nil
	}
}

// streamResult represents either a stream item or an error/end signal.
type streamResult struct {
	data []byte // nil if this is an error or stream end
	err  error  // nil for data items, ErrStreamClosed for end, or the actual error
}

// ============================================================================
// PacketTransport (low-level)
// ============================================================================

// PacketTransport is the low-level interface for sending and receiving binary messages.
// Used for transports like TCP, WebSocket, and serial that provide a bidirectional
// stream of discrete messages. Implementations must be safe for concurrent use.
type PacketTransport interface {
	// Send sends a binary message. It blocks until the message is sent.
	Send(ctx context.Context, data []byte) error
	// Recv receives a binary message. It blocks until a message is received.
	Recv(ctx context.Context) ([]byte, error)
}

// ============================================================================
// PacketClient (adapter: PacketTransport -> ClientTransport)
// ============================================================================

// pendingRequest tracks an in-flight request.
type pendingRequest struct {
	respChan   chan []byte       // For unary responses
	streamChan chan streamResult // For streaming responses (items, errors, and end)
	errChan    chan error        // For unary errors
	isStream   bool
}

// PacketClient multiplexes RPC calls over a PacketTransport.
// Implements ClientTransport.
type PacketClient struct {
	transport   PacketTransport
	mu          sync.Mutex
	nextID      uint64
	pending     map[uint64]*pendingRequest
	recvRunning bool
	recvErr     error
	recvErrMu   sync.RWMutex
}

// NewPacketClient creates a new PacketClient.
func NewPacketClient(transport PacketTransport) *PacketClient {
	return &PacketClient{
		transport: transport,
		nextID:    1,
		pending:   make(map[uint64]*pendingRequest),
	}
}

// Run runs the client's receive loop until the context is canceled or the transport fails.
// Must be called before making any RPC calls. This method blocks until an error occurs.
func (c *PacketClient) Run(ctx context.Context) error {
	c.mu.Lock()
	if c.recvRunning {
		c.mu.Unlock()
		return errors.New("client already running")
	}
	c.recvRunning = true
	c.mu.Unlock()

	return c.receiveLoop(ctx)
}

func (c *PacketClient) receiveLoop(ctx context.Context) error {
	for {
		select {
		case <-ctx.Done():
			c.setRecvError(ctx.Err())
			return ctx.Err()
		default:
		}

		data, err := c.transport.Recv(ctx)
		if err != nil {
			c.setRecvError(err)
			return err
		}

		c.handleResponse(data)
	}
}

func (c *PacketClient) setRecvError(err error) {
	c.recvErrMu.Lock()
	c.recvErr = err
	c.recvErrMu.Unlock()

	// Notify all pending requests
	c.mu.Lock()
	for _, req := range c.pending {
		if req.isStream {
			select {
			case req.streamChan <- streamResult{err: err}:
			default:
			}
		} else {
			select {
			case req.errChan <- err:
			default:
			}
		}
	}
	c.mu.Unlock()
}

func (c *PacketClient) handleResponse(data []byte) {
	buf := data

	if len(buf) == 0 {
		return
	}
	msgType := buf[0]
	buf = buf[1:]

	requestID, err := DecodeLEB128(&buf)
	if err != nil {
		return
	}

	c.mu.Lock()
	req, ok := c.pending[requestID]
	c.mu.Unlock()

	if !ok {
		return
	}

	switch msgType {
	case rpcTypeResponse:
		payload := make([]byte, len(buf))
		copy(payload, buf)
		select {
		case req.respChan <- payload:
		default:
		}

	case rpcTypeStreamItem:
		payload := make([]byte, len(buf))
		copy(payload, buf)
		select {
		case req.streamChan <- streamResult{data: payload}:
		default:
		}

	case rpcTypeStreamEnd:
		select {
		case req.streamChan <- streamResult{err: ErrStreamClosed}:
		default:
		}
		c.mu.Lock()
		delete(c.pending, requestID)
		c.mu.Unlock()

	case rpcTypeError:
		errCode, _ := DecodeLEB128(&buf)
		errMsg, _ := DecodeString(&buf)
		rpcErr := &RpcError{Code: uint32(errCode), Message: errMsg}
		if req.isStream {
			select {
			case req.streamChan <- streamResult{err: rpcErr}:
			default:
			}
		} else {
			select {
			case req.errChan <- rpcErr:
			default:
			}
		}
		c.mu.Lock()
		delete(c.pending, requestID)
		c.mu.Unlock()
	}
}

// CallUnary makes a unary RPC call.
func (c *PacketClient) CallUnary(ctx context.Context, methodIndex uint32, payload []byte) ([]byte, error) {
	c.recvErrMu.RLock()
	if c.recvErr != nil {
		err := c.recvErr
		c.recvErrMu.RUnlock()
		return nil, err
	}
	c.recvErrMu.RUnlock()

	c.mu.Lock()
	requestID := c.nextID
	c.nextID++

	req := &pendingRequest{
		respChan: make(chan []byte, 1),
		errChan:  make(chan error, 1),
		isStream: false,
	}
	c.pending[requestID] = req
	c.mu.Unlock()

	var buf []byte
	buf = append(buf, rpcTypeRequest)
	EncodeLEB128(requestID, &buf)
	EncodeLEB128(uint64(methodIndex), &buf)
	buf = append(buf, payload...)

	if err := c.transport.Send(ctx, buf); err != nil {
		c.mu.Lock()
		delete(c.pending, requestID)
		c.mu.Unlock()
		return nil, err
	}

	select {
	case <-ctx.Done():
		c.sendCancel(requestID)
		c.mu.Lock()
		delete(c.pending, requestID)
		c.mu.Unlock()
		return nil, ctx.Err()
	case resp := <-req.respChan:
		c.mu.Lock()
		delete(c.pending, requestID)
		c.mu.Unlock()
		return resp, nil
	case err := <-req.errChan:
		return nil, err
	}
}

// CallStream makes a streaming RPC call.
func (c *PacketClient) CallStream(ctx context.Context, methodIndex uint32, payload []byte) (*StreamReceiver, error) {
	c.recvErrMu.RLock()
	if c.recvErr != nil {
		err := c.recvErr
		c.recvErrMu.RUnlock()
		return nil, err
	}
	c.recvErrMu.RUnlock()

	c.mu.Lock()
	requestID := c.nextID
	c.nextID++

	streamChan := make(chan streamResult, 16)
	req := &pendingRequest{
		streamChan: streamChan,
		isStream:   true,
	}
	c.pending[requestID] = req
	c.mu.Unlock()

	var buf []byte
	buf = append(buf, rpcTypeRequest)
	EncodeLEB128(requestID, &buf)
	EncodeLEB128(uint64(methodIndex), &buf)
	buf = append(buf, payload...)

	if err := c.transport.Send(ctx, buf); err != nil {
		c.mu.Lock()
		delete(c.pending, requestID)
		c.mu.Unlock()
		return nil, err
	}

	return &StreamReceiver{
		streamChan: streamChan,
		ctx:        ctx,
		cancelFunc: func() {
			c.sendCancel(requestID)
			c.mu.Lock()
			delete(c.pending, requestID)
			c.mu.Unlock()
		},
	}, nil
}

func (c *PacketClient) sendCancel(requestID uint64) {
	var buf []byte
	buf = append(buf, rpcTypeCancel)
	EncodeLEB128(requestID, &buf)
	_ = c.transport.Send(context.Background(), buf)
}

// ============================================================================
// PacketServer (adapter: PacketTransport -> ServerTransport)
// ============================================================================

// PacketServerCall represents a single incoming RPC request over a packet transport.
type PacketServerCall struct {
	methodIndex uint32
	payload     []byte
	transport   PacketTransport
	callID      uint64
	ctx         context.Context
	cancel      context.CancelFunc
}

// MethodIndex returns the method index.
func (c *PacketServerCall) MethodIndex() uint32 {
	return c.methodIndex
}

// Payload returns the request payload.
func (c *PacketServerCall) Payload() []byte {
	return c.payload
}

// Context returns the context for this call.
func (c *PacketServerCall) Context() context.Context {
	return c.ctx
}

// SendResponse sends a unary response.
func (c *PacketServerCall) SendResponse(payload []byte) error {
	var buf []byte
	buf = append(buf, rpcTypeResponse)
	EncodeLEB128(c.callID, &buf)
	buf = append(buf, payload...)
	return c.transport.Send(context.Background(), buf)
}

// SendStreamItem sends a stream item.
func (c *PacketServerCall) SendStreamItem(payload []byte) error {
	var buf []byte
	buf = append(buf, rpcTypeStreamItem)
	EncodeLEB128(c.callID, &buf)
	buf = append(buf, payload...)
	return c.transport.Send(context.Background(), buf)
}

// SendStreamEnd signals the end of a stream.
func (c *PacketServerCall) SendStreamEnd() error {
	var buf []byte
	buf = append(buf, rpcTypeStreamEnd)
	EncodeLEB128(c.callID, &buf)
	return c.transport.Send(context.Background(), buf)
}

// SendError sends an error response.
func (c *PacketServerCall) SendError(code uint32, message string) error {
	var buf []byte
	buf = append(buf, rpcTypeError)
	EncodeLEB128(c.callID, &buf)
	EncodeLEB128(uint64(code), &buf)
	EncodeString(message, &buf)
	return c.transport.Send(context.Background(), buf)
}

// PacketServer demultiplexes RPC calls from a PacketTransport.
type PacketServer struct {
	transport   PacketTransport
	handler     ServerHandler
	mu          sync.Mutex
	activeCalls map[uint64]context.CancelFunc
}

// NewPacketServer creates a new PacketServer.
func NewPacketServer(transport PacketTransport, handler ServerHandler) *PacketServer {
	return &PacketServer{
		transport:   transport,
		handler:     handler,
		activeCalls: make(map[uint64]context.CancelFunc),
	}
}

// Run runs the server's receive loop. It blocks until the context is canceled or the transport fails.
func (s *PacketServer) Run(ctx context.Context) error {
	for {
		select {
		case <-ctx.Done():
			return ctx.Err()
		default:
		}

		data, err := s.transport.Recv(ctx)
		if err != nil {
			return err
		}

		buf := data
		if len(buf) == 0 {
			continue
		}
		msgType := buf[0]
		buf = buf[1:]

		callID, err := DecodeLEB128(&buf)
		if err != nil {
			continue
		}

		switch msgType {
		case rpcTypeRequest:
			methodIndex, err := DecodeLEB128(&buf)
			if err != nil {
				callCtx, callCancel := context.WithCancel(ctx)
				call := &PacketServerCall{
					transport: s.transport,
					callID:    callID,
					ctx:       callCtx,
					cancel:    callCancel,
				}
				call.SendError(ErrCodeDecodeError, "failed to decode method index")
				callCancel()
				continue
			}

			callCtx, callCancel := context.WithCancel(ctx)
			call := &PacketServerCall{
				methodIndex: uint32(methodIndex),
				payload:     buf,
				transport:   s.transport,
				callID:      callID,
				ctx:         callCtx,
				cancel:      callCancel,
			}

			s.mu.Lock()
			s.activeCalls[callID] = callCancel
			s.mu.Unlock()

			go s.handler.HandleCall(call)
		case rpcTypeCancel:
			s.mu.Lock()
			if cancel, ok := s.activeCalls[callID]; ok {
				cancel()
				delete(s.activeCalls, callID)
			}
			s.mu.Unlock()
		}
	}
}

// ============================================================================
// TCP PacketTransport
// ============================================================================

// TCPTransport implements PacketTransport over a TCP-like connection (net.Conn).
// It uses LEB128 length-prefix framing as per the RPC transport spec.
type TCPTransport struct {
	conn interface {
		Read(b []byte) (n int, err error)
		Write(b []byte) (n int, err error)
	}
	readMu  sync.Mutex
	writeMu sync.Mutex
}

// NewTCPTransport creates a new TCPTransport from a net.Conn.
func NewTCPTransport(conn interface {
	Read(b []byte) (n int, err error)
	Write(b []byte) (n int, err error)
}) *TCPTransport {
	return &TCPTransport{conn: conn}
}

// Send sends a binary message with LEB128 length-prefix framing.
func (t *TCPTransport) Send(ctx context.Context, data []byte) error {
	t.writeMu.Lock()
	defer t.writeMu.Unlock()

	var lenBuf []byte
	EncodeLEB128(uint64(len(data)), &lenBuf)
	if _, err := t.conn.Write(lenBuf); err != nil {
		return err
	}
	_, err := t.conn.Write(data)
	return err
}

// Recv receives a binary message with LEB128 length-prefix framing.
func (t *TCPTransport) Recv(ctx context.Context) ([]byte, error) {
	t.readMu.Lock()
	defer t.readMu.Unlock()

	length, err := t.readLEB128()
	if err != nil {
		return nil, err
	}

	data := make([]byte, length)
	if _, err := t.readFull(data); err != nil {
		return nil, err
	}
	return data, nil
}

func (t *TCPTransport) readLEB128() (uint64, error) {
	var result uint64
	var shift uint
	for {
		var b [1]byte
		if _, err := t.conn.Read(b[:]); err != nil {
			return 0, err
		}
		result |= uint64(b[0]&0x7F) << shift
		if b[0]&0x80 == 0 {
			break
		}
		shift += 7
	}
	return result, nil
}

func (t *TCPTransport) readFull(buf []byte) (int, error) {
	total := 0
	for total < len(buf) {
		n, err := t.conn.Read(buf[total:])
		if err != nil {
			return total + n, err
		}
		total += n
	}
	return total, nil
}

// ============================================================================
// WebSocket PacketTransport
// ============================================================================

// WebSocketTransport implements PacketTransport over a WebSocket connection.
// Each WebSocket binary message is one packet (no additional framing needed).
type WebSocketTransport struct {
	conn    WebSocketConn
	recvCh  chan []byte
	errCh   chan error
	closeCh chan struct{}
	once    sync.Once
	writeMu sync.Mutex
}

// WebSocketConn is the interface required from a WebSocket connection.
type WebSocketConn interface {
	ReadMessage() (messageType int, p []byte, err error)
	WriteMessage(messageType int, data []byte) error
	Close() error
}

// NewWebSocketTransport creates a new WebSocket transport.
func NewWebSocketTransport(conn WebSocketConn) *WebSocketTransport {
	t := &WebSocketTransport{
		conn:    conn,
		recvCh:  make(chan []byte, 100),
		errCh:   make(chan error, 1),
		closeCh: make(chan struct{}),
	}
	go t.readLoop()
	return t
}

func (t *WebSocketTransport) readLoop() {
	for {
		messageType, message, err := t.conn.ReadMessage()
		if err != nil {
			select {
			case t.errCh <- err:
			case <-t.closeCh:
			}
			return
		}
		// Only accept binary messages (type 2)
		if messageType != 2 {
			continue
		}
		select {
		case t.recvCh <- message:
		case <-t.closeCh:
			return
		}
	}
}

// Send sends a binary WebSocket message.
func (t *WebSocketTransport) Send(ctx context.Context, data []byte) error {
	select {
	case <-t.closeCh:
		return fmt.Errorf("transport closed")
	case <-ctx.Done():
		return ctx.Err()
	default:
	}
	t.writeMu.Lock()
	defer t.writeMu.Unlock()
	return t.conn.WriteMessage(2, data) // 2 = BinaryMessage
}

// Recv receives a binary WebSocket message.
func (t *WebSocketTransport) Recv(ctx context.Context) ([]byte, error) {
	select {
	case <-ctx.Done():
		return nil, ctx.Err()
	case <-t.closeCh:
		return nil, fmt.Errorf("transport closed")
	case err := <-t.errCh:
		return nil, err
	case data := <-t.recvCh:
		return data, nil
	}
}

// Close closes the transport.
func (t *WebSocketTransport) Close() error {
	t.once.Do(func() {
		close(t.closeCh)
	})
	return t.conn.Close()
}

// ============================================================================
// HTTP Client (implements ClientTransport directly)
// ============================================================================

// Content types for HTTP RPC
const (
	contentTypeRPC       = "application/x-volex-rpc"
	contentTypeRPCStream = "application/x-volex-rpc-stream"
	contentTypeRPCError  = "application/x-volex-rpc-error"
)

// HttpClient implements ClientTransport over HTTP.
// Each RPC call maps to a single HTTP POST request.
type HttpClient struct {
	url    string
	client *http.Client
}

// NewHttpClient creates a new HTTP RPC client.
func NewHttpClient(url string) *HttpClient {
	return &HttpClient{
		url:    url,
		client: &http.Client{},
	}
}

// CallUnary makes a unary RPC call over HTTP.
func (c *HttpClient) CallUnary(ctx context.Context, methodIndex uint32, payload []byte) ([]byte, error) {
	// Build request body: [method_index: LEB128] [payload]
	var body []byte
	EncodeLEB128(uint64(methodIndex), &body)
	body = append(body, payload...)

	req, err := http.NewRequestWithContext(ctx, "POST", c.url, bytes.NewReader(body))
	if err != nil {
		return nil, err
	}
	req.Header.Set("Content-Type", contentTypeRPC)

	resp, err := c.client.Do(req)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()

	respBody, err := io.ReadAll(resp.Body)
	if err != nil {
		return nil, err
	}

	// Check for error responses
	if resp.StatusCode != http.StatusOK {
		return nil, parseHttpError(resp.StatusCode, resp.Header.Get("Content-Type"), respBody)
	}

	return respBody, nil
}

// CallStream makes a streaming RPC call over HTTP.
func (c *HttpClient) CallStream(ctx context.Context, methodIndex uint32, payload []byte) (*StreamReceiver, error) {
	// Build request body: [method_index: LEB128] [payload]
	var body []byte
	EncodeLEB128(uint64(methodIndex), &body)
	body = append(body, payload...)

	req, err := http.NewRequestWithContext(ctx, "POST", c.url, bytes.NewReader(body))
	if err != nil {
		return nil, err
	}
	req.Header.Set("Content-Type", contentTypeRPC)

	resp, err := c.client.Do(req)
	if err != nil {
		return nil, err
	}

	// Check for error responses (non-200)
	if resp.StatusCode != http.StatusOK {
		defer resp.Body.Close()
		respBody, err := io.ReadAll(resp.Body)
		if err != nil {
			return nil, err
		}
		return nil, parseHttpError(resp.StatusCode, resp.Header.Get("Content-Type"), respBody)
	}

	// Stream response: read LEB128-framed messages from body
	streamChan := make(chan streamResult, 16)

	go func() {
		defer resp.Body.Close()
		reader := resp.Body
		for {
			// Read LEB128 length
			length, err := readLEB128FromReader(reader)
			if err != nil {
				if err == io.EOF || err == io.ErrUnexpectedEOF {
					streamChan <- streamResult{err: ErrStreamClosed}
				} else {
					streamChan <- streamResult{err: err}
				}
				return
			}

			// Read message
			msg := make([]byte, length)
			if _, err := io.ReadFull(reader, msg); err != nil {
				streamChan <- streamResult{err: err}
				return
			}

			if len(msg) == 0 {
				streamChan <- streamResult{err: errors.New("empty stream message")}
				return
			}

			msgType := msg[0]
			msgPayload := msg[1:]

			switch msgType {
			case rpcTypeStreamItem:
				streamChan <- streamResult{data: msgPayload}
			case rpcTypeStreamEnd:
				streamChan <- streamResult{err: ErrStreamClosed}
				return
			case rpcTypeError:
				buf := msgPayload
				errCode, _ := DecodeLEB128(&buf)
				errMsg, _ := DecodeString(&buf)
				streamChan <- streamResult{err: &RpcError{Code: uint32(errCode), Message: errMsg}}
				return
			default:
				streamChan <- streamResult{err: fmt.Errorf("unknown stream message type: 0x%02x", msgType)}
				return
			}
		}
	}()

	return &StreamReceiver{
		streamChan: streamChan,
		ctx:        ctx,
		cancelFunc: func() {
			resp.Body.Close()
		},
	}, nil
}

func parseHttpError(statusCode int, contentType string, body []byte) error {
	if contentType == contentTypeRPCError && len(body) > 0 {
		buf := body
		errCode, _ := DecodeLEB128(&buf)
		errMsg, _ := DecodeString(&buf)
		return &RpcError{Code: uint32(errCode), Message: errMsg}
	}
	return &RpcError{Code: ErrCodeHandlerError, Message: fmt.Sprintf("HTTP %d", statusCode)}
}

func readLEB128FromReader(r io.Reader) (uint64, error) {
	var result uint64
	var shift uint
	var b [1]byte
	for {
		_, err := r.Read(b[:])
		if err != nil {
			return 0, err
		}
		result |= uint64(b[0]&0x7F) << shift
		if b[0]&0x80 == 0 {
			break
		}
		shift += 7
	}
	return result, nil
}

// ============================================================================
// HTTP Server
// ============================================================================

// HttpServerCall represents a single incoming HTTP RPC request.
type HttpServerCall struct {
	methodIndex  uint32
	payload      []byte
	ctx          context.Context
	w            http.ResponseWriter
	flusher      http.Flusher
	streaming    bool
	wroteHeader  bool
}

// MethodIndex returns the method index.
func (c *HttpServerCall) MethodIndex() uint32 {
	return c.methodIndex
}

// Payload returns the request payload.
func (c *HttpServerCall) Payload() []byte {
	return c.payload
}

// Context returns the context for this call.
func (c *HttpServerCall) Context() context.Context {
	return c.ctx
}

// SendResponse sends a unary response.
func (c *HttpServerCall) SendResponse(payload []byte) error {
	c.w.Header().Set("Content-Type", contentTypeRPC)
	c.w.WriteHeader(http.StatusOK)
	_, err := c.w.Write(payload)
	return err
}

// SendStreamItem sends a stream item.
func (c *HttpServerCall) SendStreamItem(payload []byte) error {
	if !c.streaming {
		c.streaming = true
		c.w.Header().Set("Content-Type", contentTypeRPCStream)
		c.w.WriteHeader(http.StatusOK)
		c.wroteHeader = true
	}
	return c.writeStreamFrame(rpcTypeStreamItem, payload)
}

// SendStreamEnd signals the end of a stream.
func (c *HttpServerCall) SendStreamEnd() error {
	if !c.streaming {
		c.streaming = true
		c.w.Header().Set("Content-Type", contentTypeRPCStream)
		c.w.WriteHeader(http.StatusOK)
		c.wroteHeader = true
	}
	return c.writeStreamFrame(rpcTypeStreamEnd, nil)
}

// SendError sends an error response.
func (c *HttpServerCall) SendError(code uint32, message string) error {
	if c.streaming {
		// Already started streaming — send error as a stream frame
		var msg []byte
		EncodeLEB128(uint64(code), &msg)
		EncodeString(message, &msg)
		return c.writeStreamFrame(rpcTypeError, msg)
	}
	// Not yet streaming — send as a normal HTTP error response
	statusCode := http.StatusInternalServerError
	switch code {
	case ErrCodeUnknownMethod:
		statusCode = http.StatusNotFound
	case ErrCodeDecodeError:
		statusCode = http.StatusBadRequest
	}
	c.w.Header().Set("Content-Type", contentTypeRPCError)
	c.w.WriteHeader(statusCode)
	var errBody []byte
	EncodeLEB128(uint64(code), &errBody)
	EncodeString(message, &errBody)
	_, err := c.w.Write(errBody)
	return err
}

func (c *HttpServerCall) writeStreamFrame(msgType byte, payload []byte) error {
	// Message: [type] [payload]
	var msg []byte
	msg = append(msg, msgType)
	msg = append(msg, payload...)

	// Frame: [length: LEB128] [message]
	var frame []byte
	EncodeLEB128(uint64(len(msg)), &frame)
	frame = append(frame, msg...)

	if _, err := c.w.Write(frame); err != nil {
		return err
	}
	c.flusher.Flush()
	return nil
}

// HttpServer serves RPC calls over HTTP.
type HttpServer struct {
	handler ServerHandler
}

// NewHttpServer creates a new HTTP RPC server.
func NewHttpServer(handler ServerHandler) *HttpServer {
	return &HttpServer{
		handler: handler,
	}
}

// Handler returns an http.Handler that processes RPC requests.
func (s *HttpServer) Handler() http.Handler {
	return http.HandlerFunc(s.serveHTTP)
}

func (s *HttpServer) serveHTTP(w http.ResponseWriter, r *http.Request) {
	if r.Method != "POST" {
		http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
		return
	}

	// Read full body
	body, err := io.ReadAll(r.Body)
	if err != nil {
		http.Error(w, "failed to read body", http.StatusBadRequest)
		return
	}
	r.Body.Close()

	// Parse: [method_index: LEB128] [payload]
	buf := body
	methodIndex, err := DecodeLEB128(&buf)
	if err != nil {
		http.Error(w, "failed to decode method index", http.StatusBadRequest)
		return
	}

	flusher, isFlusher := w.(http.Flusher)
	if !isFlusher {
		http.Error(w, "streaming not supported", http.StatusInternalServerError)
		return
	}

	call := &HttpServerCall{
		methodIndex: uint32(methodIndex),
		payload:     buf,
		ctx:         r.Context(),
		w:           w,
		flusher:     flusher,
	}

	s.handler.HandleCall(call)
}

