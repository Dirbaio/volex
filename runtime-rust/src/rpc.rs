//! RPC infrastructure for volex services.

use std::cell::RefCell;
use std::collections::HashMap;
use std::future::Future;
use std::marker::PhantomData;
use std::rc::Rc;

use tokio::sync::{mpsc, oneshot};

use crate::{DecodeError, Encode, decode_leb128_u64, encode_leb128_u64};

// ============================================================================
// Utilities
// ============================================================================

/// A guard that runs a closure when dropped.
struct OnDrop<F: FnOnce()>(Option<F>);

impl<F: FnOnce()> OnDrop<F> {
    fn new(f: F) -> Self {
        Self(Some(f))
    }

    /// Defuses the guard, preventing the closure from running on drop.
    fn defuse(&mut self) {
        self.0.take();
    }
}

impl<F: FnOnce()> Drop for OnDrop<F> {
    fn drop(&mut self) {
        if let Some(f) = self.0.take() {
            f();
        }
    }
}

/// Spawns a local async task that can be canceled. Used by generated server code.
///
/// The `cancel_rx` is signaled when the client sends a CANCEL message for this call.
/// When canceled, the handler future is dropped (triggering any drop guards).
pub fn spawn_cancellable(cancel_rx: oneshot::Receiver<()>, fut: impl Future<Output = ()> + 'static) {
    tokio::task::spawn_local(async move {
        tokio::select! {
            _ = fut => {}
            _ = cancel_rx => {}
        }
    });
}

// ============================================================================
// RPC Errors
// ============================================================================

/// Error codes for RPC errors.
pub const ERR_CODE_UNKNOWN_METHOD: u32 = 1;
pub const ERR_CODE_DECODE_ERROR: u32 = 2;
pub const ERR_CODE_HANDLER_ERROR: u32 = 3;

/// RPC error type.
#[derive(Debug, Clone)]
pub struct RpcError {
    pub code: u32,
    pub message: String,
}

impl RpcError {
    /// Creates a new RPC error.
    pub fn new(code: u32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    /// Creates an RPC error from a decode error.
    pub fn decode(e: DecodeError) -> Self {
        Self {
            code: ERR_CODE_DECODE_ERROR,
            message: format!("decode error: {}", e),
        }
    }

    /// Creates an RPC error for stream closed.
    pub fn stream_closed() -> Self {
        Self {
            code: 0,
            message: "stream closed".to_string(),
        }
    }

    /// Returns true if this is a stream closed error.
    pub fn is_stream_closed(&self) -> bool {
        self.code == 0 && self.message == "stream closed"
    }
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "RPC error {}: {}", self.code, self.message)
    }
}

impl std::error::Error for RpcError {}

// ============================================================================
// RPC Message Types
// ============================================================================

// Server -> Client message types (0x00-0x7F)
const RPC_TYPE_RESPONSE: u8 = 0x00;
const RPC_TYPE_STREAM_ITEM: u8 = 0x01;
const RPC_TYPE_STREAM_END: u8 = 0x02;
const RPC_TYPE_ERROR: u8 = 0x03;

// Client -> Server message types (0x80-0xFF)
const RPC_TYPE_REQUEST: u8 = 0x80;
const RPC_TYPE_CANCEL: u8 = 0x81;

// ============================================================================
// High-level Transport Interfaces
// ============================================================================

/// High-level client transport trait.
///
/// Provides per-call RPC operations. Generated client code uses this trait.
/// Implementations include [`PacketClient`] (for packet-based transports like TCP)
/// and HTTP clients.
pub trait ClientTransport {
    /// Makes a unary RPC call.
    fn call_unary(&self, method_index: u32, payload: Vec<u8>) -> impl Future<Output = Result<Vec<u8>, RpcError>>;

    /// Makes a streaming RPC call. Returns a stream receiver.
    fn call_stream(&self, method_index: u32, payload: Vec<u8>) -> impl Future<Output = Result<StreamReceiver, RpcError>>;
}

impl<T: ClientTransport> ClientTransport for Rc<T> {
    fn call_unary(&self, method_index: u32, payload: Vec<u8>) -> impl Future<Output = Result<Vec<u8>, RpcError>> {
        (**self).call_unary(method_index, payload)
    }

    fn call_stream(&self, method_index: u32, payload: Vec<u8>) -> impl Future<Output = Result<StreamReceiver, RpcError>> {
        (**self).call_stream(method_index, payload)
    }
}

/// High-level server transport trait.
///
/// Accepts incoming RPC calls. Generated server code uses this trait.
/// Implementations include [`PacketServer`] (for packet-based transports like TCP)
/// and HTTP servers.
pub trait ServerTransport {
    /// The type of incoming call.
    type Call: ServerCall;

    /// Accepts the next incoming RPC call.
    fn accept(&self) -> impl Future<Output = Result<Self::Call, RpcError>>;
}

/// A single incoming RPC request (server-side).
pub trait ServerCall {
    /// Returns the method index.
    fn method_index(&self) -> u32;

    /// Returns the request payload.
    fn payload(&self) -> &[u8];

    /// Takes the cancellation receiver for this call.
    fn take_cancel_rx(&mut self) -> oneshot::Receiver<()>;

    /// Sends a unary response. Consumes the call.
    fn send_response(self, payload: Vec<u8>) -> impl Future<Output = Result<(), RpcError>>;

    /// Converts this call into a stream sender for streaming responses.
    fn into_stream_sender(self) -> StreamSenderBase;

    /// Sends an error response. Consumes the call.
    fn send_error(self, code: u32, message: &str) -> impl Future<Output = Result<(), RpcError>>;
}

// ============================================================================
// Stream Sender (server-side)
// ============================================================================

/// Stream sender for streaming responses (server-side, typed).
///
/// Wraps a `StreamSenderBase` to provide typed sending.
pub struct StreamSender<T: Encode> {
    base: StreamSenderBase,
    _phantom: PhantomData<T>,
}

impl<T: Encode> StreamSender<T> {
    /// Creates a new typed stream sender.
    pub fn new(base: StreamSenderBase) -> Self {
        Self {
            base,
            _phantom: PhantomData,
        }
    }

    /// Sends an item to the stream.
    ///
    /// Returns an error if the stream has been closed (e.g., due to transport error).
    pub async fn send(&self, item: T) -> Result<(), RpcError> {
        let mut buf = Vec::new();
        item.encode(&mut buf);
        self.base.send(buf).await
    }

    /// Marks the stream as finished with an error.
    /// After calling this, no StreamEnd will be sent on drop.
    pub async fn error(self, code: u32, message: &str) {
        self.base.error(code, message).await;
    }
}

/// Stream sender for server-side streaming responses (untyped).
///
/// Sends stream items, errors, and end-of-stream signals through the transport.
/// Sends StreamEnd on drop if not already finished.
///
/// The sender uses a channel that carries "stream messages" — each message is
/// a framed payload with a type tag:
/// - `[STREAM_ITEM] [body]`
/// - `[STREAM_END]`
/// - `[ERROR] [code: LEB128] [message: string]`
///
/// The receiver of this channel is responsible for further framing (e.g., adding
/// call_id for packet transports).
pub struct StreamSenderBase {
    tx: mpsc::Sender<Vec<u8>>,
    finished: bool,
}

impl StreamSenderBase {
    /// Creates a new stream sender base.
    pub fn new(tx: mpsc::Sender<Vec<u8>>) -> Self {
        Self { tx, finished: false }
    }

    /// Sends a stream item (already encoded payload).
    pub async fn send(&self, payload: Vec<u8>) -> Result<(), RpcError> {
        let mut buf = Vec::new();
        buf.push(RPC_TYPE_STREAM_ITEM);
        buf.extend_from_slice(&payload);
        self.tx
            .send(buf)
            .await
            .map_err(|_| RpcError::new(0, "transport closed"))
    }

    /// Marks the stream as finished with an error.
    /// After calling this, no StreamEnd will be sent on drop.
    pub async fn error(mut self, code: u32, message: &str) {
        self.finished = true;
        let mut buf = Vec::new();
        buf.push(RPC_TYPE_ERROR);
        encode_leb128_u64(code as u64, &mut buf);
        encode_leb128_u64(message.len() as u64, &mut buf);
        buf.extend_from_slice(message.as_bytes());
        let _ = self.tx.send(buf).await;
    }
}

impl Drop for StreamSenderBase {
    fn drop(&mut self) {
        if !self.finished {
            let mut buf = Vec::new();
            buf.push(RPC_TYPE_STREAM_END);
            let _ = self.tx.try_send(buf);
        }
    }
}

/// Stream receiver for streaming responses (client-side).
///
/// Cancels the stream on drop if not already closed.
pub struct StreamReceiver {
    rx: mpsc::Receiver<StreamEvent>,
    cancel_tx: Option<oneshot::Sender<()>>,
}

/// Stream item from server - either data or error.
enum StreamEvent {
    Item(Vec<u8>),
    End,
    Error(RpcError),
}

impl StreamReceiver {
    /// Receives the next item from the stream.
    pub async fn recv(&mut self) -> Result<Vec<u8>, RpcError> {
        match self.rx.recv().await {
            Some(StreamEvent::Item(data)) => Ok(data),
            Some(StreamEvent::End) => Err(RpcError::stream_closed()),
            Some(StreamEvent::Error(e)) => Err(e),
            None => Err(RpcError::stream_closed()),
        }
    }
}

impl Drop for StreamReceiver {
    fn drop(&mut self) {
        // Signal cancellation
        if let Some(tx) = self.cancel_tx.take() {
            let _ = tx.send(());
        }
    }
}

// ============================================================================
// PacketTransport (low-level)
// ============================================================================

/// Low-level packet transport trait for sending and receiving binary messages.
///
/// This is used for transports like TCP, WebSocket, and serial that provide
/// a bidirectional stream of discrete messages. The multiplexing of multiple
/// RPC calls over a single connection is handled by [`PacketClient`] and
/// [`PacketServer`], which wrap a `PacketTransport`.
pub trait PacketTransport {
    /// Sends a binary message.
    fn send(&self, data: Vec<u8>) -> impl Future<Output = Result<(), RpcError>>;
    /// Receives a binary message.
    fn recv(&self) -> impl Future<Output = Result<Vec<u8>, RpcError>>;
}

// ============================================================================
// PacketClient (adapter: PacketTransport -> ClientTransport)
// ============================================================================

/// Pending request tracking.
enum PendingRequest {
    Unary {
        resp_tx: oneshot::Sender<Result<Vec<u8>, RpcError>>,
    },
    Stream {
        stream_tx: mpsc::Sender<StreamEvent>,
    },
}

/// Client adapter that multiplexes RPC calls over a [`PacketTransport`].
///
/// Implements [`ClientTransport`] by handling call ID allocation, request/response
/// matching, and cancellation over a single packet-based connection.
pub struct PacketClient<Tr: PacketTransport> {
    transport: Tr,
    next_id: RefCell<u64>,
    pending: Rc<RefCell<HashMap<u64, PendingRequest>>>,
    tx_send: mpsc::Sender<Vec<u8>>,
    tx_recv: RefCell<Option<mpsc::Receiver<Vec<u8>>>>,
}

impl<Tr: PacketTransport> PacketClient<Tr> {
    /// Creates a new packet client.
    pub fn new(transport: Tr) -> Self {
        let (tx_send, tx_recv) = mpsc::channel(64);
        Self {
            transport,
            next_id: RefCell::new(1),
            pending: Rc::new(RefCell::new(HashMap::new())),
            tx_send,
            tx_recv: RefCell::new(Some(tx_recv)),
        }
    }

    /// Runs the client's send and receive loops.
    ///
    /// This function runs until the transport is closed or an error occurs.
    /// Call this from within a `LocalSet` context.
    pub async fn run(&self) -> Result<(), RpcError> {
        let mut tx_recv = self.tx_recv.borrow_mut().take().expect("run() called twice");

        // Guard to notify all pending requests when run() exits (error or drop)
        let _pending_guard = OnDrop::new({
            let pending = self.pending.clone();
            move || {
                let err = RpcError::new(0, "transport closed");
                for (_, req) in pending.borrow_mut().drain() {
                    match req {
                        PendingRequest::Unary { resp_tx } => {
                            let _ = resp_tx.send(Err(err.clone()));
                        }
                        PendingRequest::Stream { stream_tx } => {
                            let _ = stream_tx.try_send(StreamEvent::Error(err.clone()));
                        }
                    }
                }
            }
        });

        // Receive loop - runs until transport error
        let rx_loop = async {
            loop {
                let data = self.transport.recv().await?;

                let mut buf = data.as_slice();

                // Decode message type
                if buf.is_empty() {
                    continue; // Invalid message, ignore
                }
                let msg_type = buf[0];
                buf = &buf[1..];

                // Decode request ID
                let request_id = match decode_leb128_u64(&mut buf) {
                    Ok(id) => id,
                    Err(_) => continue, // Invalid message, ignore
                };

                let mut pending = self.pending.borrow_mut();
                let req = match pending.get_mut(&request_id) {
                    Some(req) => req,
                    None => continue, // Unknown request ID, ignore
                };

                match msg_type {
                    RPC_TYPE_RESPONSE => {
                        if let PendingRequest::Unary { .. } = req {
                            if let Some(PendingRequest::Unary { resp_tx }) = pending.remove(&request_id) {
                                let _ = resp_tx.send(Ok(buf.to_vec()));
                            }
                        }
                    }
                    RPC_TYPE_STREAM_ITEM => {
                        if let PendingRequest::Stream { stream_tx } = req {
                            let _ = stream_tx.send(StreamEvent::Item(buf.to_vec())).await;
                        }
                    }
                    RPC_TYPE_STREAM_END => {
                        if let PendingRequest::Stream { .. } = req {
                            if let Some(PendingRequest::Stream { stream_tx }) = pending.remove(&request_id) {
                                let _ = stream_tx.send(StreamEvent::End).await;
                            }
                        }
                    }
                    RPC_TYPE_ERROR => {
                        let err_code = decode_leb128_u64(&mut buf).unwrap_or(0) as u32;
                        let err_len = decode_leb128_u64(&mut buf).unwrap_or(0) as usize;
                        let err_msg = if buf.len() >= err_len {
                            String::from_utf8_lossy(&buf[..err_len]).to_string()
                        } else {
                            "unknown error".to_string()
                        };
                        let err = RpcError::new(err_code, err_msg);
                        match pending.remove(&request_id) {
                            Some(PendingRequest::Unary { resp_tx }) => {
                                let _ = resp_tx.send(Err(err));
                            }
                            Some(PendingRequest::Stream { stream_tx }) => {
                                let _ = stream_tx.send(StreamEvent::Error(err)).await;
                            }
                            None => {}
                        }
                    }
                    _ => {
                        // Unknown message type, ignore
                    }
                }
            }
            #[allow(unreachable_code)]
            Ok::<(), RpcError>(())
        };

        // Send loop - runs until channel closed or transport error
        let tx_loop = async {
            while let Some(packet) = tx_recv.recv().await {
                self.transport.send(packet).await?;
            }
            Ok::<(), RpcError>(())
        };

        // Run both loops, return first error
        tokio::try_join!(rx_loop, tx_loop).map(|_| ())
    }
}

impl<Tr: PacketTransport> ClientTransport for PacketClient<Tr> {
    async fn call_unary(&self, method_index: u32, payload: Vec<u8>) -> Result<Vec<u8>, RpcError> {
        // Allocate request ID
        let request_id = {
            let mut next_id = self.next_id.borrow_mut();
            let id = *next_id;
            *next_id += 1;
            id
        };

        // Create response channel
        let (resp_tx, resp_rx) = oneshot::channel();

        // Register pending request
        self.pending
            .borrow_mut()
            .insert(request_id, PendingRequest::Unary { resp_tx });

        // Build request message
        let mut buf = Vec::new();
        buf.push(RPC_TYPE_REQUEST);
        encode_leb128_u64(request_id, &mut buf);
        encode_leb128_u64(method_index as u64, &mut buf);
        buf.extend_from_slice(&payload);

        // Send request via channel
        if self.tx_send.send(buf).await.is_err() {
            self.pending.borrow_mut().remove(&request_id);
            return Err(RpcError::new(0, "transport closed"));
        }

        // Guard to send cancel message if dropped
        let mut guard = OnDrop::new({
            let tx_send = self.tx_send.clone();
            let pending = self.pending.clone();
            move || {
                pending.borrow_mut().remove(&request_id);
                let mut buf = Vec::new();
                buf.push(RPC_TYPE_CANCEL);
                encode_leb128_u64(request_id, &mut buf);
                let _ = tx_send.try_send(buf);
            }
        });

        // Wait for response
        let result = resp_rx.await.map_err(|_| RpcError::new(0, "response channel closed"))?;
        guard.defuse();
        result
    }

    async fn call_stream(&self, method_index: u32, payload: Vec<u8>) -> Result<StreamReceiver, RpcError> {
        // Allocate request ID
        let request_id = {
            let mut next_id = self.next_id.borrow_mut();
            let id = *next_id;
            *next_id += 1;
            id
        };

        // Create stream channel
        let (stream_tx, stream_rx) = mpsc::channel(16);

        // Register pending request
        self.pending
            .borrow_mut()
            .insert(request_id, PendingRequest::Stream { stream_tx });

        // Build request message
        let mut buf = Vec::new();
        buf.push(RPC_TYPE_REQUEST);
        encode_leb128_u64(request_id, &mut buf);
        encode_leb128_u64(method_index as u64, &mut buf);
        buf.extend_from_slice(&payload);

        // Send request via channel
        if self.tx_send.send(buf).await.is_err() {
            self.pending.borrow_mut().remove(&request_id);
            return Err(RpcError::new(0, "transport closed"));
        }

        // Create cancel channel
        let (cancel_tx, cancel_rx) = oneshot::channel();

        // Spawn task to handle cancellation
        {
            let tx_send = self.tx_send.clone();
            let pending = self.pending.clone();
            tokio::task::spawn_local(async move {
                if cancel_rx.await.is_ok() {
                    pending.borrow_mut().remove(&request_id);
                    let mut buf = Vec::new();
                    buf.push(RPC_TYPE_CANCEL);
                    encode_leb128_u64(request_id, &mut buf);
                    let _ = tx_send.try_send(buf);
                }
            });
        }

        Ok(StreamReceiver {
            rx: stream_rx,
            cancel_tx: Some(cancel_tx),
        })
    }
}

// ============================================================================
// PacketServer (adapter: PacketTransport -> ServerTransport)
// ============================================================================

/// A single incoming RPC request over a packet transport.
pub struct PacketServerCall {
    method_index: u32,
    payload: Vec<u8>,
    tx: mpsc::Sender<Vec<u8>>,
    call_id: u64,
    cancel_rx: oneshot::Receiver<()>,
}

impl ServerCall for PacketServerCall {
    fn method_index(&self) -> u32 {
        self.method_index
    }

    fn payload(&self) -> &[u8] {
        &self.payload
    }

    fn take_cancel_rx(&mut self) -> oneshot::Receiver<()> {
        let (_, dummy_rx) = oneshot::channel();
        std::mem::replace(&mut self.cancel_rx, dummy_rx)
    }

    async fn send_response(self, payload: Vec<u8>) -> Result<(), RpcError> {
        let mut buf = Vec::new();
        buf.push(RPC_TYPE_RESPONSE);
        encode_leb128_u64(self.call_id, &mut buf);
        buf.extend_from_slice(&payload);
        self.tx
            .send(buf)
            .await
            .map_err(|_| RpcError::new(0, "transport closed"))
    }

    fn into_stream_sender(self) -> StreamSenderBase {
        // Create a forwarding channel that prepends call_id to each stream message
        let (stream_tx, mut stream_rx) = mpsc::channel::<Vec<u8>>(16);
        let out_tx = self.tx;
        let call_id = self.call_id;
        tokio::task::spawn_local(async move {
            while let Some(msg) = stream_rx.recv().await {
                // Prepend call_id after the message type byte
                let mut buf = Vec::new();
                buf.push(msg[0]); // message type
                encode_leb128_u64(call_id, &mut buf);
                buf.extend_from_slice(&msg[1..]); // rest of payload
                if out_tx.send(buf).await.is_err() {
                    break;
                }
            }
        });
        StreamSenderBase::new(stream_tx)
    }

    async fn send_error(self, code: u32, message: &str) -> Result<(), RpcError> {
        let mut buf = Vec::new();
        buf.push(RPC_TYPE_ERROR);
        encode_leb128_u64(self.call_id, &mut buf);
        encode_leb128_u64(code as u64, &mut buf);
        encode_leb128_u64(message.len() as u64, &mut buf);
        buf.extend_from_slice(message.as_bytes());
        self.tx
            .send(buf)
            .await
            .map_err(|_| RpcError::new(0, "transport closed"))
    }
}

/// Server adapter that demultiplexes RPC calls from a [`PacketTransport`].
///
/// Implements [`ServerTransport`] by handling call ID parsing, cancellation,
/// and concurrent request management over a single packet-based connection.
pub struct PacketServer<Tr: PacketTransport> {
    transport: Tr,
    call_rx: RefCell<mpsc::Receiver<PacketServerCall>>,
    call_tx: mpsc::Sender<PacketServerCall>,
    // Channel for outgoing packets
    out_tx: mpsc::Sender<Vec<u8>>,
    out_rx: RefCell<Option<mpsc::Receiver<Vec<u8>>>>,
    // Active call cancel senders, indexed by call ID
    cancel_txs: RefCell<HashMap<u64, oneshot::Sender<()>>>,
}

impl<Tr: PacketTransport> PacketServer<Tr> {
    /// Creates a new packet server.
    pub fn new(transport: Tr) -> Self {
        let (call_tx, call_rx) = mpsc::channel(64);
        let (out_tx, out_rx) = mpsc::channel(64);
        Self {
            transport,
            call_rx: RefCell::new(call_rx),
            call_tx,
            out_tx,
            out_rx: RefCell::new(Some(out_rx)),
            cancel_txs: RefCell::new(HashMap::new()),
        }
    }

    /// Runs the server's receive and send loops.
    ///
    /// This function must be run concurrently with the code that calls `accept()`.
    /// It runs until the transport is closed or an error occurs.
    pub async fn run(&self) -> Result<(), RpcError> {
        let mut out_rx = self.out_rx.borrow_mut().take().expect("run() called twice");

        // Receive loop - reads packets and dispatches to accept()
        let rx_loop = async {
            loop {
                let data = self.transport.recv().await?;

                let mut buf = data.as_slice();

                if buf.is_empty() {
                    continue;
                }
                let msg_type = buf[0];
                buf = &buf[1..];

                let call_id = match decode_leb128_u64(&mut buf) {
                    Ok(id) => id,
                    Err(_) => continue,
                };

                match msg_type {
                    RPC_TYPE_REQUEST => {
                        let method_index = match decode_leb128_u64(&mut buf) {
                            Ok(idx) => idx as u32,
                            Err(_) => {
                                let mut err_buf = Vec::new();
                                err_buf.push(RPC_TYPE_ERROR);
                                encode_leb128_u64(call_id, &mut err_buf);
                                encode_leb128_u64(ERR_CODE_DECODE_ERROR as u64, &mut err_buf);
                                let msg = "failed to decode method index";
                                encode_leb128_u64(msg.len() as u64, &mut err_buf);
                                err_buf.extend_from_slice(msg.as_bytes());
                                let _ = self.out_tx.send(err_buf).await;
                                continue;
                            }
                        };

                        let (cancel_tx, cancel_rx) = oneshot::channel();
                        let call = PacketServerCall {
                            method_index,
                            payload: buf.to_vec(),
                            tx: self.out_tx.clone(),
                            call_id,
                            cancel_rx,
                        };

                        self.cancel_txs.borrow_mut().insert(call_id, cancel_tx);

                        if self.call_tx.send(call).await.is_err() {
                            break;
                        }
                    }
                    RPC_TYPE_CANCEL => {
                        if let Some(cancel_tx) = self.cancel_txs.borrow_mut().remove(&call_id) {
                            let _ = cancel_tx.send(());
                        }
                    }
                    _ => {}
                }
            }
            Ok::<(), RpcError>(())
        };

        // Send loop
        let tx_loop = async {
            while let Some(packet) = out_rx.recv().await {
                self.transport.send(packet).await?;
            }
            Ok::<(), RpcError>(())
        };

        tokio::try_join!(rx_loop, tx_loop).map(|_| ())
    }
}

impl<Tr: PacketTransport> ServerTransport for PacketServer<Tr> {
    type Call = PacketServerCall;

    async fn accept(&self) -> Result<PacketServerCall, RpcError> {
        self.call_rx
            .borrow_mut()
            .recv()
            .await
            .ok_or_else(|| RpcError::new(0, "transport closed"))
    }
}

// ============================================================================
// TCP PacketTransport
// ============================================================================

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};

/// TCP transport for RPC.
pub struct TcpTransport {
    read: RefCell<OwnedReadHalf>,
    write: RefCell<OwnedWriteHalf>,
}

impl TcpTransport {
    /// Creates a new TCP transport from a TCP stream.
    pub fn new(stream: tokio::net::TcpStream) -> Self {
        // Disable Nagle's algorithm for lower latency
        let _ = stream.set_nodelay(true);
        let (read, write) = stream.into_split();
        Self {
            read: RefCell::new(read),
            write: RefCell::new(write),
        }
    }
}

impl PacketTransport for TcpTransport {
    async fn send(&self, data: Vec<u8>) -> Result<(), RpcError> {
        let mut write = self.write.borrow_mut();

        // Write length prefix as LEB128
        let mut len_buf = Vec::new();
        encode_leb128_u64(data.len() as u64, &mut len_buf);
        write
            .write_all(&len_buf)
            .await
            .map_err(|e| RpcError::new(0, e.to_string()))?;
        write
            .write_all(&data)
            .await
            .map_err(|e| RpcError::new(0, e.to_string()))?;
        write.flush().await.map_err(|e| RpcError::new(0, e.to_string()))?;
        Ok(())
    }

    async fn recv(&self) -> Result<Vec<u8>, RpcError> {
        let mut read = self.read.borrow_mut();

        // Read length prefix as LEB128
        let mut length: u64 = 0;
        let mut shift = 0;
        loop {
            let mut byte = [0u8; 1];
            read.read_exact(&mut byte)
                .await
                .map_err(|e| RpcError::new(0, e.to_string()))?;
            length |= ((byte[0] & 0x7F) as u64) << shift;
            if byte[0] & 0x80 == 0 {
                break;
            }
            shift += 7;
        }

        // Read data
        let mut data = vec![0u8; length as usize];
        read.read_exact(&mut data)
            .await
            .map_err(|e| RpcError::new(0, e.to_string()))?;
        Ok(data)
    }
}

// ============================================================================
// HTTP Transport
// ============================================================================

#[cfg(feature = "http")]
mod http_transport {
    use std::pin::Pin;
    use std::task::{Context, Poll};

    use bytes::Bytes;
    use http_body_util::{BodyExt, Full};
    use hyper::body::{Body, Frame, Incoming};
    use hyper::service::Service;
    use hyper_util::rt::TokioIo;

    use super::*;

    const CONTENT_TYPE_RPC: &str = "application/x-volex-rpc";
    const CONTENT_TYPE_RPC_STREAM: &str = "application/x-volex-rpc-stream";
    const CONTENT_TYPE_RPC_ERROR: &str = "application/x-volex-rpc-error";

    // ========================================================================
    // HTTP Client
    // ========================================================================

    /// HTTP RPC client. Each call maps to a single HTTP POST request.
    pub struct HttpClient {
        host: String,
        port: u16,
        path: String,
    }

    impl HttpClient {
        /// Creates a new HTTP client from a URL like `http://host:port/path`.
        pub fn new(url: &str) -> Self {
            let url = url.strip_prefix("http://").unwrap_or(url);
            let (host_port, path) = match url.find('/') {
                Some(i) => (&url[..i], &url[i..]),
                None => (url, "/rpc"),
            };
            let (host, port) = match host_port.find(':') {
                Some(i) => (&host_port[..i], host_port[i + 1..].parse::<u16>().unwrap_or(80)),
                None => (host_port, 80),
            };
            Self {
                host: host.to_string(),
                port,
                path: path.to_string(),
            }
        }

        async fn do_request(
            &self,
            body: Vec<u8>,
        ) -> Result<hyper::Response<Incoming>, RpcError> {
            let stream = tokio::net::TcpStream::connect((&*self.host, self.port))
                .await
                .map_err(|e| RpcError::new(0, e.to_string()))?;
            let _ = stream.set_nodelay(true);
            let io = TokioIo::new(stream);

            let (mut sender, conn) = hyper::client::conn::http1::handshake(io)
                .await
                .map_err(|e| RpcError::new(0, e.to_string()))?;

            tokio::task::spawn_local(async move {
                let _ = conn.await;
            });

            let req = hyper::Request::builder()
                .method("POST")
                .uri(&self.path)
                .header("host", format!("{}:{}", self.host, self.port))
                .header("content-type", CONTENT_TYPE_RPC)
                .header("connection", "close")
                .body(Full::new(Bytes::from(body)))
                .map_err(|e| RpcError::new(0, e.to_string()))?;

            sender
                .send_request(req)
                .await
                .map_err(|e| RpcError::new(0, e.to_string()))
        }
    }

    fn get_header(resp: &hyper::Response<Incoming>, name: &str) -> String {
        resp.headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string()
    }

    fn parse_http_error(content_type: &str, body: &[u8]) -> RpcError {
        if content_type == CONTENT_TYPE_RPC_ERROR && !body.is_empty() {
            let mut buf = body;
            if let Ok(code) = decode_leb128_u64(&mut buf) {
                if let Ok(msg_len) = decode_leb128_u64(&mut buf) {
                    if buf.len() >= msg_len as usize {
                        let msg = String::from_utf8_lossy(&buf[..msg_len as usize]).to_string();
                        return RpcError::new(code as u32, msg);
                    }
                }
            }
        }
        RpcError::new(ERR_CODE_HANDLER_ERROR, "HTTP error")
    }

    fn parse_stream_events(data: &[u8], event_tx: &mpsc::Sender<StreamEvent>) {
        let mut buf = data;
        loop {
            if buf.is_empty() {
                let _ = event_tx.try_send(StreamEvent::End);
                return;
            }
            let length = match decode_leb128_u64(&mut buf) {
                Ok(l) => l as usize,
                Err(_) => {
                    let _ = event_tx.try_send(StreamEvent::Error(RpcError::new(0, "invalid LEB128 in stream")));
                    return;
                }
            };
            if buf.len() < length || length == 0 {
                let _ = event_tx.try_send(StreamEvent::Error(RpcError::new(0, "truncated stream message")));
                return;
            }
            let msg = &buf[..length];
            buf = &buf[length..];

            let msg_type = msg[0];
            let msg_payload = &msg[1..];

            match msg_type {
                RPC_TYPE_STREAM_ITEM => {
                    if event_tx.try_send(StreamEvent::Item(msg_payload.to_vec())).is_err() {
                        return;
                    }
                }
                RPC_TYPE_STREAM_END => {
                    let _ = event_tx.try_send(StreamEvent::End);
                    return;
                }
                RPC_TYPE_ERROR => {
                    let mut ebuf = msg_payload;
                    let code = decode_leb128_u64(&mut ebuf).unwrap_or(0) as u32;
                    let msg_len = decode_leb128_u64(&mut ebuf).unwrap_or(0) as usize;
                    let msg = if ebuf.len() >= msg_len {
                        String::from_utf8_lossy(&ebuf[..msg_len]).to_string()
                    } else {
                        "unknown error".to_string()
                    };
                    let _ = event_tx.try_send(StreamEvent::Error(RpcError::new(code, msg)));
                    return;
                }
                _ => {
                    let _ = event_tx.try_send(StreamEvent::Error(RpcError::new(0, "unknown stream message type")));
                    return;
                }
            }
        }
    }

    impl ClientTransport for HttpClient {
        async fn call_unary(&self, method_index: u32, payload: Vec<u8>) -> Result<Vec<u8>, RpcError> {
            let mut body = Vec::new();
            encode_leb128_u64(method_index as u64, &mut body);
            body.extend_from_slice(&payload);

            let resp = self.do_request(body).await?;
            let status = resp.status();
            let content_type = get_header(&resp, "content-type");

            let body = resp
                .into_body()
                .collect()
                .await
                .map_err(|e| RpcError::new(0, e.to_string()))?
                .to_bytes();

            if !status.is_success() {
                return Err(parse_http_error(&content_type, &body));
            }

            Ok(body.to_vec())
        }

        async fn call_stream(&self, method_index: u32, payload: Vec<u8>) -> Result<StreamReceiver, RpcError> {
            let mut body = Vec::new();
            encode_leb128_u64(method_index as u64, &mut body);
            body.extend_from_slice(&payload);

            let resp = self.do_request(body).await?;
            let status = resp.status();
            let content_type = get_header(&resp, "content-type");

            // Collect the full body
            let full_body = resp
                .into_body()
                .collect()
                .await
                .map_err(|e| RpcError::new(0, e.to_string()))?
                .to_bytes();

            if !status.is_success() {
                return Err(parse_http_error(&content_type, &full_body));
            }

            let (event_tx, event_rx) = mpsc::channel(16);
            let (cancel_tx, _cancel_rx) = oneshot::channel();

            // Parse stream messages from the collected body
            parse_stream_events(&full_body, &event_tx);

            Ok(StreamReceiver {
                rx: event_rx,
                cancel_tx: Some(cancel_tx),
            })
        }
    }

    // ========================================================================
    // HTTP Server
    // ========================================================================

    /// A single incoming HTTP RPC request.
    pub struct HttpServerCall {
        method_index: u32,
        payload: Vec<u8>,
        cancel_rx: oneshot::Receiver<()>,
        response_tx: oneshot::Sender<HttpServerResponse>,
    }

    enum HttpServerResponse {
        Unary(Vec<u8>),
        Error { code: u32, message: String },
        Stream(mpsc::Receiver<Vec<u8>>),
    }

    impl ServerCall for HttpServerCall {
        fn method_index(&self) -> u32 {
            self.method_index
        }

        fn payload(&self) -> &[u8] {
            &self.payload
        }

        fn take_cancel_rx(&mut self) -> oneshot::Receiver<()> {
            let (_, dummy_rx) = oneshot::channel();
            std::mem::replace(&mut self.cancel_rx, dummy_rx)
        }

        async fn send_response(self, payload: Vec<u8>) -> Result<(), RpcError> {
            let _ = self.response_tx.send(HttpServerResponse::Unary(payload));
            Ok(())
        }

        fn into_stream_sender(self) -> StreamSenderBase {
            let (stream_tx, stream_rx) = mpsc::channel::<Vec<u8>>(16);
            let _ = self.response_tx.send(HttpServerResponse::Stream(stream_rx));
            StreamSenderBase::new(stream_tx)
        }

        async fn send_error(self, code: u32, message: &str) -> Result<(), RpcError> {
            let _ = self.response_tx.send(HttpServerResponse::Error {
                code,
                message: message.to_string(),
            });
            Ok(())
        }
    }

    /// A streaming response body that reads from an mpsc channel.
    struct StreamBody {
        rx: mpsc::Receiver<Vec<u8>>,
    }

    impl Body for StreamBody {
        type Data = Bytes;
        type Error = std::convert::Infallible;

        fn poll_frame(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
        ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
            match self.rx.poll_recv(cx) {
                Poll::Ready(Some(msg)) => {
                    // LEB128-frame the message
                    let mut frame = Vec::new();
                    encode_leb128_u64(msg.len() as u64, &mut frame);
                    frame.extend_from_slice(&msg);
                    Poll::Ready(Some(Ok(Frame::data(Bytes::from(frame)))))
                }
                Poll::Ready(None) => Poll::Ready(None),
                Poll::Pending => Poll::Pending,
            }
        }
    }

    /// Either a fixed body or a streaming body.
    enum ResponseBody {
        Full(http_body_util::Full<Bytes>),
        Stream(StreamBody),
    }

    impl Body for ResponseBody {
        type Data = Bytes;
        type Error = std::convert::Infallible;

        fn poll_frame(
            self: Pin<&mut Self>,
            cx: &mut Context<'_>,
        ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
            match self.get_mut() {
                ResponseBody::Full(b) => Pin::new(b).poll_frame(cx).map_err(|e| match e {}),
                ResponseBody::Stream(b) => Pin::new(b).poll_frame(cx),
            }
        }
    }

    /// Hyper service that dispatches RPC calls.
    struct RpcService {
        call_tx: mpsc::Sender<HttpServerCall>,
    }

    impl Service<hyper::Request<Incoming>> for RpcService {
        type Response = hyper::Response<ResponseBody>;
        type Error = std::convert::Infallible;
        type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>>>>;

        fn call(&self, req: hyper::Request<Incoming>) -> Self::Future {
            let call_tx = self.call_tx.clone();
            Box::pin(async move {
                Ok(handle_request(req, call_tx).await)
            })
        }
    }

    async fn handle_request(
        req: hyper::Request<Incoming>,
        call_tx: mpsc::Sender<HttpServerCall>,
    ) -> hyper::Response<ResponseBody> {
        if req.method() != hyper::Method::POST {
            return hyper::Response::builder()
                .status(405)
                .header("connection", "close")
                .body(ResponseBody::Full(Full::new(Bytes::new())))
                .unwrap();
        }

        // Read body
        let body = match req.into_body().collect().await {
            Ok(b) => b.to_bytes(),
            Err(_) => {
                return hyper::Response::builder()
                    .status(400)
                    .header("connection", "close")
                    .body(ResponseBody::Full(Full::new(Bytes::new())))
                    .unwrap();
            }
        };

        // Parse: [method_index: LEB128] [payload]
        let mut buf = body.as_ref();
        let method_index = match decode_leb128_u64(&mut buf) {
            Ok(idx) => idx as u32,
            Err(_) => {
                return hyper::Response::builder()
                    .status(400)
                    .header("connection", "close")
                    .body(ResponseBody::Full(Full::new(Bytes::new())))
                    .unwrap();
            }
        };
        let payload = buf.to_vec();

        // Create call
        let (response_tx, response_rx) = oneshot::channel();
        let (cancel_tx, cancel_rx) = oneshot::channel();

        let call = HttpServerCall {
            method_index,
            payload,
            cancel_rx,
            response_tx,
        };

        if call_tx.send(call).await.is_err() {
            return hyper::Response::builder()
                .status(500)
                .header("connection", "close")
                .body(ResponseBody::Full(Full::new(Bytes::new())))
                .unwrap();
        }

        // Wait for response
        let resp = match response_rx.await {
            Ok(r) => r,
            Err(_) => {
                let _ = cancel_tx.send(());
                return hyper::Response::builder()
                    .status(500)
                    .header("connection", "close")
                    .body(ResponseBody::Full(Full::new(Bytes::new())))
                    .unwrap();
            }
        };

        match resp {
            HttpServerResponse::Unary(payload) => hyper::Response::builder()
                .status(200)
                .header("content-type", CONTENT_TYPE_RPC)
                .header("connection", "close")
                .body(ResponseBody::Full(Full::new(Bytes::from(payload))))
                .unwrap(),
            HttpServerResponse::Error { code, message } => {
                let status = match code {
                    ERR_CODE_UNKNOWN_METHOD => 404,
                    ERR_CODE_DECODE_ERROR => 400,
                    _ => 500,
                };
                let mut err_body = Vec::new();
                encode_leb128_u64(code as u64, &mut err_body);
                encode_leb128_u64(message.len() as u64, &mut err_body);
                err_body.extend_from_slice(message.as_bytes());
                hyper::Response::builder()
                    .status(status)
                    .header("content-type", CONTENT_TYPE_RPC_ERROR)
                    .header("connection", "close")
                    .body(ResponseBody::Full(Full::new(Bytes::from(err_body))))
                    .unwrap()
            }
            HttpServerResponse::Stream(stream_rx) => hyper::Response::builder()
                .status(200)
                .header("content-type", CONTENT_TYPE_RPC_STREAM)
                .header("connection", "close")
                .body(ResponseBody::Stream(StreamBody { rx: stream_rx }))
                .unwrap(),
        }
    }

    /// HTTP RPC server. Listens on a TCP port and accepts RPC calls.
    pub struct HttpServer {
        call_rx: RefCell<mpsc::Receiver<HttpServerCall>>,
    }

    impl HttpServer {
        /// Creates a new HTTP server and starts listening.
        pub async fn new(listener: tokio::net::TcpListener) -> Self {
            let (call_tx, call_rx) = mpsc::channel(64);
            let server = Self {
                call_rx: RefCell::new(call_rx),
            };

            tokio::task::spawn_local(async move {
                loop {
                    let (stream, _) = match listener.accept().await {
                        Ok(v) => v,
                        Err(_) => return,
                    };
                    let _ = stream.set_nodelay(true);
                    let io = TokioIo::new(stream);
                    let svc = RpcService {
                        call_tx: call_tx.clone(),
                    };
                    tokio::task::spawn_local(async move {
                        let _ = hyper::server::conn::http1::Builder::new()
                            .serve_connection(io, svc)
                            .await;
                    });
                }
            });

            server
        }
    }

    impl ServerTransport for HttpServer {
        type Call = HttpServerCall;

        async fn accept(&self) -> Result<HttpServerCall, RpcError> {
            self.call_rx
                .borrow_mut()
                .recv()
                .await
                .ok_or_else(|| RpcError::new(0, "server closed"))
        }
    }
}

#[cfg(feature = "http")]
pub use http_transport::{HttpClient, HttpServer, HttpServerCall};
