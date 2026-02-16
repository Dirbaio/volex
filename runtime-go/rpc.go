// Package volex provides runtime encoding/decoding support.
package volex

import (
	"context"
	"errors"
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

// ServerTransport accepts incoming RPC calls.
// Generated server code uses this interface.
type ServerTransport interface {
	// Accept waits for and returns the next incoming RPC call.
	Accept(ctx context.Context) (ServerCall, error)
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
// Implements ServerTransport.
type PacketServer struct {
	transport   PacketTransport
	callChan    chan ServerCall
	mu          sync.Mutex
	activeCalls map[uint64]context.CancelFunc
}

// NewPacketServer creates a new PacketServer.
func NewPacketServer(transport PacketTransport) *PacketServer {
	return &PacketServer{
		transport:   transport,
		callChan:    make(chan ServerCall, 64),
		activeCalls: make(map[uint64]context.CancelFunc),
	}
}

// Run runs the server's receive loop. Must be called concurrently with Accept.
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

			select {
			case s.callChan <- call:
			case <-ctx.Done():
				callCancel()
				return ctx.Err()
			}
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

// Accept waits for and returns the next incoming RPC call.
func (s *PacketServer) Accept(ctx context.Context) (ServerCall, error) {
	select {
	case call := <-s.callChan:
		return call, nil
	case <-ctx.Done():
		return nil, ctx.Err()
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
