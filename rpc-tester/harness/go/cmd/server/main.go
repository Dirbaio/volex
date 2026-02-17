package main

import (
	"context"
	"errors"
	"fmt"
	"log"
	"net"
	"net/http"
	"os"
	"sync"
	"time"

	"github.com/gorilla/websocket"
	"rpc_test/gen"

	volex "github.com/volex/runtime"
)

// TestServiceImpl implements the TestService interface
type TestServiceImpl struct {
	mu         sync.Mutex
	lastStatus string
}

func (s *TestServiceImpl) Echo(ctx context.Context, req gen.EchoRequest) (gen.EchoResponse, error) {
	return gen.EchoResponse{Text: req.Text}, nil
}

func (s *TestServiceImpl) Subscribe(ctx context.Context, req gen.StreamRequest, stream gen.TestServiceSubscribeStream) error {
	for i := uint32(0); i < req.Count; i++ {
		err := stream.Send(gen.StreamItem{
			Seq:  i,
			Data: fmt.Sprintf("item-%d", i),
		})
		if err != nil {
			return err
		}
	}
	return nil
}

func (s *TestServiceImpl) MaybeFail(ctx context.Context, req gen.FailRequest) (gen.FailResponse, error) {
	if req.ShouldFail {
		return gen.FailResponse{}, errors.New(req.ErrorMessage)
	}
	return gen.FailResponse{Success: true}, nil
}

func (s *TestServiceImpl) StreamThenFail(ctx context.Context, req gen.StreamRequest, stream gen.TestServiceStreamThenFailStream) error {
	// Send some items, then fail
	for i := uint32(0); i < req.Count; i++ {
		err := stream.Send(gen.StreamItem{
			Seq:  i,
			Data: fmt.Sprintf("item-%d", i),
		})
		if err != nil {
			return err
		}
	}
	return errors.New("stream error after items")
}

func (s *TestServiceImpl) Add(ctx context.Context, req uint32) (uint32, error) {
	return req + 10, nil
}

func (s *TestServiceImpl) GetStrings(ctx context.Context, req uint32, stream gen.TestServiceGetStringsStream) error {
	for i := uint32(0); i < req; i++ {
		err := stream.Send(fmt.Sprintf("string-%d", i))
		if err != nil {
			return err
		}
	}
	return nil
}

func (s *TestServiceImpl) SlowUnary(ctx context.Context, req gen.SlowRequest) (gen.SlowResponse, error) {
	s.mu.Lock()
	s.lastStatus = "slow_unary: started"
	s.mu.Unlock()
	// Wait for the specified delay, but respect context cancellation
	select {
	case <-time.After(time.Duration(req.DelayMs) * time.Millisecond):
		s.mu.Lock()
		s.lastStatus = "slow_unary: completed"
		s.mu.Unlock()
		return gen.SlowResponse{Completed: true}, nil
	case <-ctx.Done():
		s.mu.Lock()
		s.lastStatus = "slow_unary: canceled"
		s.mu.Unlock()
		return gen.SlowResponse{}, ctx.Err()
	}
}

func (s *TestServiceImpl) SlowStream(ctx context.Context, req gen.SlowRequest, stream gen.TestServiceSlowStreamStream) error {
	s.mu.Lock()
	s.lastStatus = "slow_stream: started"
	s.mu.Unlock()
	// Send items with delay between each, respecting cancellation
	for i := uint32(0); ; i++ {
		select {
		case <-time.After(time.Duration(req.DelayMs) * time.Millisecond):
			err := stream.Send(gen.StreamItem{
				Seq:  i,
				Data: fmt.Sprintf("slow-item-%d", i),
			})
			if err != nil {
				return err
			}
		case <-ctx.Done():
			s.mu.Lock()
			s.lastStatus = "slow_stream: canceled"
			s.mu.Unlock()
			return ctx.Err()
		}
	}
}

func (s *TestServiceImpl) GetStatus(ctx context.Context, req gen.Empty) (string, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.lastStatus, nil
}

func serveTCP() {
	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		fmt.Fprintf(os.Stderr, "failed to listen: %v\n", err)
		os.Exit(1)
	}

	// Print the address for the client to connect to
	fmt.Println(listener.Addr().String())

	ctx := context.Background()

	for {
		conn, err := listener.Accept()
		if err != nil {
			return
		}
		go func(conn net.Conn) {
			defer conn.Close()
			transport := volex.NewTCPTransport(conn)
			server := volex.NewPacketServer(transport)
			impl := &TestServiceImpl{}
			go server.Run(ctx)
			gen.ServeTestService(ctx, server, impl)
		}(conn)
	}
}

func serveHTTP() {
	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		fmt.Fprintf(os.Stderr, "failed to listen: %v\n", err)
		os.Exit(1)
	}

	ctx := context.Background()

	httpServer := volex.NewHttpServer()
	impl_ := &TestServiceImpl{}

	// Start accepting RPC calls in background
	go gen.ServeTestService(ctx, httpServer, impl_)

	// Print the URL for the client to connect to
	fmt.Printf("http://%s/rpc\n", listener.Addr().String())

	// Serve HTTP
	mux := http.NewServeMux()
	mux.Handle("/rpc", httpServer.Handler())
	srv := &http.Server{Handler: mux}
	srv.Serve(listener)
}

var wsUpgrader = websocket.Upgrader{
	CheckOrigin: func(r *http.Request) bool { return true },
}

func serveWS() {
	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		fmt.Fprintf(os.Stderr, "failed to listen: %v\n", err)
		os.Exit(1)
	}

	ctx := context.Background()

	mux := http.NewServeMux()
	mux.HandleFunc("/rpc", func(w http.ResponseWriter, r *http.Request) {
		conn, err := wsUpgrader.Upgrade(w, r, nil)
		if err != nil {
			return
		}
		transport := volex.NewWebSocketTransport(conn)
		server := volex.NewPacketServer(transport)
		impl := &TestServiceImpl{}
		go server.Run(ctx)
		gen.ServeTestService(ctx, server, impl)
	})

	fmt.Printf("ws://%s/rpc\n", listener.Addr().String())

	srv := &http.Server{Handler: mux}
	srv.Serve(listener)
}

func main() {
	transport := os.Getenv("TRANSPORT")
	if transport == "" {
		transport = "tcp"
	}

	switch transport {
	case "tcp":
		serveTCP()
	case "http":
		serveHTTP()
	case "ws":
		serveWS()
	default:
		log.Fatalf("unknown transport: %s", transport)
	}
}
