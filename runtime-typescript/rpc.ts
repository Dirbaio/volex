// Volex RPC Client Infrastructure for TypeScript
// This implements the client-side RPC protocol as defined in doc/rpc_transport.md

import { Buf, WriteBuf, encodeVarint, decodeVarint } from './volex.js';

// ============================================================================
// RPC Message Types
// ============================================================================

// Server -> Client message types (0x00-0x7F)
const RPC_TYPE_RESPONSE = 0x00;
const RPC_TYPE_STREAM_ITEM = 0x01;
const RPC_TYPE_STREAM_END = 0x02;
const RPC_TYPE_ERROR = 0x03;

// Client -> Server message types (0x80-0xFF)
const RPC_TYPE_REQUEST = 0x80;
const RPC_TYPE_CANCEL = 0x81;

// ============================================================================
// RPC Errors
// ============================================================================

export const ERR_CODE_UNKNOWN_METHOD = 1;
export const ERR_CODE_DECODE_ERROR = 2;
export const ERR_CODE_HANDLER_ERROR = 3;

export class RpcError extends Error {
  code: number;

  constructor(code: number, message: string) {
    super(message);
    this.code = code;
    this.name = 'RpcError';
  }

  isStreamClosed(): boolean {
    return this.code === 0 && this.message === 'stream closed';
  }

  isStreamCancelled(): boolean {
    return this.code === 0 && this.message === 'stream cancelled';
  }

  static streamClosed(): RpcError {
    return new RpcError(0, 'stream closed');
  }

  static streamCancelled(): RpcError {
    return new RpcError(0, 'stream cancelled');
  }
}

// ============================================================================
// Transport
// ============================================================================

export interface PacketTransport {
  send(data: Uint8Array): Promise<void>;
  recv(): Promise<Uint8Array>;
  close(): void;
}

// ============================================================================
// TCP Transport (for Node.js)
// ============================================================================

export class TcpTransport implements PacketTransport {
  private socket: any; // net.Socket
  private buffer: Uint8Array = new Uint8Array(0);
  private waiters: Array<{
    resolve: (value: Uint8Array) => void;
    reject: (error: Error) => void;
  }> = [];
  private closed = false;
  private error: Error | null = null;

  constructor(socket: any) {
    this.socket = socket;

    // Disable Nagle's algorithm for lower latency
    socket.setNoDelay(true);

    socket.on('data', (data: Uint8Array) => {
      // Append to buffer
      const newBuf = new Uint8Array(this.buffer.length + data.length);
      newBuf.set(this.buffer);
      newBuf.set(data, this.buffer.length);
      this.buffer = newBuf;

      // Try to process complete messages
      this.processBuffer();
    });

    socket.on('close', () => {
      this.closed = true;
      const err = new Error('connection closed');
      this.error = err;
      // Reject all pending waiters
      for (const waiter of this.waiters) {
        waiter.reject(err);
      }
      this.waiters = [];
    });

    socket.on('error', (err: Error) => {
      this.closed = true;
      this.error = err;
      // Reject all pending waiters
      for (const waiter of this.waiters) {
        waiter.reject(err);
      }
      this.waiters = [];
    });
  }

  private processBuffer(): void {
    while (this.waiters.length > 0) {
      // Try to read a complete LEB128 length + payload
      const buf = new Buf(this.buffer, 0);

      // Check if we have enough bytes to read the length
      if (buf.data.length === 0) {
        break;
      }

      // Try to decode length
      let length: number;
      const startOffset = buf.offset;
      try {
        length = decodeVarint(buf);
      } catch {
        // Not enough bytes for length
        break;
      }

      const headerLen = buf.offset - startOffset;
      const totalLen = headerLen + length;

      if (this.buffer.length < totalLen) {
        // Not enough bytes for payload
        break;
      }

      // Extract the message
      const payload = this.buffer.slice(headerLen, totalLen);
      this.buffer = this.buffer.slice(totalLen);

      // Resolve the first waiter
      const waiter = this.waiters.shift()!;
      waiter.resolve(payload);
    }
  }

  async send(data: Uint8Array): Promise<void> {
    if (this.closed) {
      throw this.error || new Error('connection closed');
    }

    // Encode length prefix as LEB128
    const lenBuf = new WriteBuf();
    encodeVarint(data.length, lenBuf);
    const lenBytes = lenBuf.toUint8Array();

    // Combine length and data
    const packet = new Uint8Array(lenBytes.length + data.length);
    packet.set(lenBytes);
    packet.set(data, lenBytes.length);

    return new Promise((resolve, reject) => {
      this.socket.write(packet, (err?: Error) => {
        if (err) {
          reject(err);
        } else {
          resolve();
        }
      });
    });
  }

  async recv(): Promise<Uint8Array> {
    if (this.closed) {
      throw this.error || new Error('connection closed');
    }

    // Check if we already have a complete message in the buffer
    if (this.buffer.length > 0) {
      const buf = new Buf(this.buffer, 0);
      try {
        const length = decodeVarint(buf);
        const headerLen = buf.offset;
        const totalLen = headerLen + length;

        if (this.buffer.length >= totalLen) {
          const payload = this.buffer.slice(headerLen, totalLen);
          this.buffer = this.buffer.slice(totalLen);
          return payload;
        }
      } catch {
        // Not enough bytes
      }
    }

    // Wait for more data
    return new Promise((resolve, reject) => {
      this.waiters.push({ resolve, reject });
    });
  }

  close(): void {
    if (!this.closed) {
      this.closed = true;
      this.socket.end();
    }
  }
}

// ============================================================================
// WebSocket Transport (for Node.js and browsers)
// ============================================================================

export class WebSocketTransport implements PacketTransport {
  private ws: WebSocket;
  private queue: Uint8Array[] = [];
  private waiters: Array<{
    resolve: (value: Uint8Array) => void;
    reject: (error: Error) => void;
  }> = [];
  private closed = false;
  private error: Error | null = null;

  constructor(ws: WebSocket) {
    this.ws = ws;
    ws.binaryType = 'arraybuffer';

    ws.addEventListener('message', (event: MessageEvent) => {
      const data = new Uint8Array(event.data as ArrayBuffer);
      if (this.waiters.length > 0) {
        const waiter = this.waiters.shift()!;
        waiter.resolve(data);
      } else {
        this.queue.push(data);
      }
    });

    ws.addEventListener('close', () => {
      this.closed = true;
      const err = new Error('websocket closed');
      this.error = err;
      for (const waiter of this.waiters) {
        waiter.reject(err);
      }
      this.waiters = [];
    });

    ws.addEventListener('error', () => {
      this.closed = true;
      const err = new Error('websocket error');
      this.error = err;
      for (const waiter of this.waiters) {
        waiter.reject(err);
      }
      this.waiters = [];
    });
  }

  async send(data: Uint8Array): Promise<void> {
    if (this.closed) {
      throw this.error || new Error('websocket closed');
    }
    this.ws.send(data);
  }

  async recv(): Promise<Uint8Array> {
    if (this.queue.length > 0) {
      return this.queue.shift()!;
    }
    if (this.closed) {
      throw this.error || new Error('websocket closed');
    }
    return new Promise((resolve, reject) => {
      this.waiters.push({ resolve, reject });
    });
  }

  close(): void {
    if (!this.closed) {
      this.closed = true;
      this.ws.close();
    }
  }
}

// ============================================================================
// High-level Transport Interfaces
// ============================================================================

export interface CallOptions {
  signal?: AbortSignal;
}

export interface ClientTransport {
  callUnary(methodIndex: number, payload: Uint8Array, options?: CallOptions): Promise<Uint8Array>;
  callStream(methodIndex: number, payload: Uint8Array, options?: CallOptions): Promise<StreamReceiver>;
}

// ============================================================================
// PacketClient (adapter: PacketTransport -> ClientTransport)
// ============================================================================

type StreamEvent =
  | { type: 'item'; data: Uint8Array }
  | { type: 'end' }
  | { type: 'error'; error: RpcError };

interface PendingUnary {
  type: 'unary';
  resolve: (data: Uint8Array) => void;
  reject: (error: RpcError) => void;
}

interface PendingStream {
  type: 'stream';
  queue: StreamEvent[];
  waiters: Array<(event: StreamEvent) => void>;
}

type PendingRequest = PendingUnary | PendingStream;

export class PacketClient implements ClientTransport {
  private transport: PacketTransport;
  private nextId = 1;
  private pending = new Map<number, PendingRequest>();
  private running = false;
  private runPromise: Promise<void> | null = null;
  private recvError: Error | null = null;

  constructor(transport: PacketTransport) {
    this.transport = transport;
  }

  async run(): Promise<void> {
    if (this.running) {
      throw new Error('run() already called');
    }
    this.running = true;

    try {
      while (true) {
        const data = await this.transport.recv();
        this.handleMessage(data);
      }
    } catch (e) {
      this.recvError = e as Error;
      // Notify all pending requests
      const err = new RpcError(0, (e as Error).message);
      for (const [, req] of this.pending) {
        if (req.type === 'unary') {
          req.reject(err);
        } else {
          const event: StreamEvent = { type: 'error', error: err };
          for (const waiter of req.waiters) {
            waiter(event);
          }
          req.waiters = [];
          req.queue.push(event);
        }
      }
      this.pending.clear();
      throw e;
    }
  }

  private handleMessage(data: Uint8Array): void {
    const buf = new Buf(data, 0);

    if (data.length === 0) {
      return; // Invalid message
    }

    const msgType = data[0];
    buf.offset = 1;

    // Decode request ID
    const requestId = decodeVarint(buf);

    const req = this.pending.get(requestId);
    if (!req) {
      return; // Unknown request ID
    }

    switch (msgType) {
      case RPC_TYPE_RESPONSE: {
        if (req.type === 'unary') {
          const payload = data.slice(buf.offset);
          this.pending.delete(requestId);
          req.resolve(payload);
        }
        break;
      }

      case RPC_TYPE_STREAM_ITEM: {
        if (req.type === 'stream') {
          const payload = data.slice(buf.offset);
          const event: StreamEvent = { type: 'item', data: payload };
          if (req.waiters.length > 0) {
            const waiter = req.waiters.shift()!;
            waiter(event);
          } else {
            req.queue.push(event);
          }
        }
        break;
      }

      case RPC_TYPE_STREAM_END: {
        if (req.type === 'stream') {
          this.pending.delete(requestId);
          const event: StreamEvent = { type: 'end' };
          if (req.waiters.length > 0) {
            const waiter = req.waiters.shift()!;
            waiter(event);
          } else {
            req.queue.push(event);
          }
        }
        break;
      }

      case RPC_TYPE_ERROR: {
        const errCode = decodeVarint(buf);
        const errLen = decodeVarint(buf);
        const errBytes = data.slice(buf.offset, buf.offset + errLen);
        const errMsg = new TextDecoder().decode(errBytes);
        const error = new RpcError(errCode, errMsg);

        this.pending.delete(requestId);

        if (req.type === 'unary') {
          req.reject(error);
        } else {
          const event: StreamEvent = { type: 'error', error };
          if (req.waiters.length > 0) {
            const waiter = req.waiters.shift()!;
            waiter(event);
          } else {
            req.queue.push(event);
          }
        }
        break;
      }
    }
  }

  async callUnary(methodIndex: number, payload: Uint8Array, options?: CallOptions): Promise<Uint8Array> {
    if (this.recvError) {
      throw new RpcError(0, this.recvError.message);
    }

    const signal = options?.signal;
    signal?.throwIfAborted();

    const requestId = this.nextId++;

    // Build request message
    const buf = new WriteBuf();
    buf.pushByte(RPC_TYPE_REQUEST);
    encodeVarint(requestId, buf);
    encodeVarint(methodIndex, buf);
    buf.push(payload);
    const packet = buf.toUint8Array();

    // Create promise for response
    const responsePromise = new Promise<Uint8Array>((resolve, reject) => {
      this.pending.set(requestId, { type: 'unary', resolve, reject });
    });

    // Send request
    try {
      await this.transport.send(packet);
    } catch (e) {
      this.pending.delete(requestId);
      throw new RpcError(0, (e as Error).message);
    }

    // If signal provided, listen for abort to cancel the request
    if (signal) {
      const onAbort = () => {
        const req = this.pending.get(requestId);
        if (req && req.type === 'unary') {
          this.pending.delete(requestId);
          req.reject(new RpcError(0, 'aborted'));
          this.sendCancel(requestId);
        }
      };
      signal.addEventListener('abort', onAbort, { once: true });
      // Clean up listener when response arrives
      responsePromise.then(
        () => signal.removeEventListener('abort', onAbort),
        () => signal.removeEventListener('abort', onAbort),
      );
    }

    // Wait for response
    return responsePromise;
  }

  async callStream(methodIndex: number, payload: Uint8Array, options?: CallOptions): Promise<StreamReceiver> {
    if (this.recvError) {
      throw new RpcError(0, this.recvError.message);
    }

    const signal = options?.signal;
    signal?.throwIfAborted();

    const requestId = this.nextId++;

    // Build request message
    const buf = new WriteBuf();
    buf.pushByte(RPC_TYPE_REQUEST);
    encodeVarint(requestId, buf);
    encodeVarint(methodIndex, buf);
    buf.push(payload);
    const packet = buf.toUint8Array();

    // Create pending stream
    const pendingStream: PendingStream = {
      type: 'stream',
      queue: [],
      waiters: [],
    };
    this.pending.set(requestId, pendingStream);

    // Send request
    try {
      await this.transport.send(packet);
    } catch (e) {
      this.pending.delete(requestId);
      throw new RpcError(0, (e as Error).message);
    }

    // If signal provided, listen for abort to cancel the stream
    if (signal) {
      const onAbort = () => {
        this.pending.delete(requestId);

        // Wake up any waiting recv() calls and enqueue for future ones
        const cancelEvent: StreamEvent = { type: 'error', error: new RpcError(0, 'aborted') };
        for (const waiter of pendingStream.waiters) {
          waiter(cancelEvent);
        }
        pendingStream.waiters = [];
        pendingStream.queue.push(cancelEvent);

        this.sendCancel(requestId);
      };
      signal.addEventListener('abort', onAbort, { once: true });
    }

    return new PacketStreamReceiver(pendingStream);
  }

  private async sendCancel(requestId: number): Promise<void> {
    const buf = new WriteBuf();
    buf.pushByte(RPC_TYPE_CANCEL);
    encodeVarint(requestId, buf);
    const packet = buf.toUint8Array();
    try {
      await this.transport.send(packet);
    } catch {
      // Best effort
    }
  }
}

export interface StreamReceiver {
  recv(): Promise<Uint8Array>;
}

class PacketStreamReceiver implements StreamReceiver {
  private pending: PendingStream;

  constructor(pending: PendingStream) {
    this.pending = pending;
  }

  async recv(): Promise<Uint8Array> {
    // Check queue first
    if (this.pending.queue.length > 0) {
      const event = this.pending.queue.shift()!;
      return this.handleEvent(event);
    }

    // Wait for event
    const event = await new Promise<StreamEvent>((resolve) => {
      this.pending.waiters.push(resolve);
    });

    return this.handleEvent(event);
  }

  private handleEvent(event: StreamEvent): Uint8Array {
    switch (event.type) {
      case 'item':
        return event.data;
      case 'end':
        throw RpcError.streamClosed();
      case 'error':
        throw event.error;
    }
  }
}

// ============================================================================
// HTTP Client (implements ClientTransport directly)
// ============================================================================

const CONTENT_TYPE_RPC = 'application/x-volex-rpc';
const CONTENT_TYPE_RPC_STREAM = 'application/x-volex-rpc-stream';
const CONTENT_TYPE_RPC_ERROR = 'application/x-volex-rpc-error';

export class HttpClient implements ClientTransport {
  private url: string;

  constructor(url: string) {
    this.url = url;
  }

  async callUnary(methodIndex: number, payload: Uint8Array, options?: CallOptions): Promise<Uint8Array> {
    const body = this.buildRequestBody(methodIndex, payload);
    const resp = await this.doRequest(body, options?.signal);

    if (resp.statusCode !== 200) {
      throw parseHttpError(resp.contentType, resp.body);
    }

    return resp.body;
  }

  async callStream(methodIndex: number, payload: Uint8Array, options?: CallOptions): Promise<StreamReceiver> {
    const reqBody = this.buildRequestBody(methodIndex, payload);

    const resp = await fetch(this.url, {
      method: 'POST',
      headers: {
        'Content-Type': CONTENT_TYPE_RPC,
      },
      body: reqBody as Uint8Array<ArrayBuffer>,
      signal: options?.signal,
    });

    if (resp.status !== 200) {
      const respBody = new Uint8Array(await resp.arrayBuffer());
      const contentType = resp.headers.get('content-type') || '';
      throw parseHttpError(contentType, respBody);
    }

    const pendingStream: PendingStream = { type: 'stream', queue: [], waiters: [] };

    // Stream the response body incrementally, parsing and forwarding
    // stream messages as they arrive.
    const reader = resp.body!.getReader();
    (async () => {
      let buf = new Uint8Array(0);

      const pushEvent = (event: StreamEvent) => {
        if (pendingStream.waiters.length > 0) {
          const waiter = pendingStream.waiters.shift()!;
          waiter(event);
        } else {
          pendingStream.queue.push(event);
        }
      };

      try {
        while (true) {
          const { done, value } = await reader.read();
          if (done) {
            pushEvent({ type: 'end' });
            return;
          }

          // Append chunk to buffer
          const newBuf = new Uint8Array(buf.length + value.length);
          newBuf.set(buf);
          newBuf.set(value, buf.length);
          buf = newBuf;

          // Try to parse as many complete messages as possible
          while (true) {
            if (buf.length === 0) break;

            const sbuf = new Buf(buf, 0);
            let length: number;
            try {
              length = decodeVarint(sbuf);
            } catch {
              break; // incomplete varint, wait for more data
            }

            const headerLen = sbuf.offset;
            if (buf.length < headerLen + length || length === 0) {
              break; // incomplete message, wait for more data
            }

            const msg = buf.slice(headerLen, headerLen + length);
            buf = buf.slice(headerLen + length);

            const msgType = msg[0];
            const msgPayload = msg.slice(1);

            let event: StreamEvent;
            switch (msgType) {
              case RPC_TYPE_STREAM_ITEM:
                event = { type: 'item', data: msgPayload };
                break;
              case RPC_TYPE_STREAM_END:
                event = { type: 'end' };
                break;
              case RPC_TYPE_ERROR: {
                const ebuf = new Buf(msgPayload, 0);
                const code = decodeVarint(ebuf);
                const msgLen = decodeVarint(ebuf);
                const msgBytes = msgPayload.slice(ebuf.offset, ebuf.offset + msgLen);
                const errMsg = new TextDecoder().decode(msgBytes);
                event = { type: 'error', error: new RpcError(code, errMsg) };
                break;
              }
              default:
                event = { type: 'error', error: new RpcError(0, `unknown stream message type: 0x${msgType.toString(16)}`) };
                break;
            }

            pushEvent(event);

            if (event.type === 'end' || event.type === 'error') {
              return;
            }
          }
        }
      } catch (e) {
        pushEvent({ type: 'error', error: new RpcError(0, (e as Error).message) });
      }
    })();

    return new HttpStreamReceiver(pendingStream);
  }

  private buildRequestBody(methodIndex: number, payload: Uint8Array): Uint8Array {
    const wbuf = new WriteBuf();
    encodeVarint(methodIndex, wbuf);
    wbuf.push(payload);
    return wbuf.toUint8Array();
  }

  private async doRequest(body: Uint8Array, signal?: AbortSignal): Promise<{ statusCode: number; contentType: string; body: Uint8Array }> {
    const resp = await fetch(this.url, {
      method: 'POST',
      headers: {
        'Content-Type': CONTENT_TYPE_RPC,
      },
      body: body as Uint8Array<ArrayBuffer>,
      signal,
    });

    const respBody = new Uint8Array(await resp.arrayBuffer());
    return {
      statusCode: resp.status,
      contentType: resp.headers.get('content-type') || '',
      body: respBody,
    };
  }
}

class HttpStreamReceiver implements StreamReceiver {
  private pending: PendingStream;

  constructor(pending: PendingStream) {
    this.pending = pending;
  }

  async recv(): Promise<Uint8Array> {
    if (this.pending.queue.length > 0) {
      const event = this.pending.queue.shift()!;
      return this.handleEvent(event);
    }

    const event = await new Promise<StreamEvent>((resolve) => {
      this.pending.waiters.push(resolve);
    });
    return this.handleEvent(event);
  }

  private handleEvent(event: StreamEvent): Uint8Array {
    switch (event.type) {
      case 'item':
        return event.data;
      case 'end':
        throw RpcError.streamClosed();
      case 'error':
        throw event.error;
    }
  }
}

function parseHttpError(contentType: string, body: Uint8Array): RpcError {
  if (contentType === CONTENT_TYPE_RPC_ERROR && body.length > 0) {
    const buf = new Buf(body, 0);
    try {
      const code = decodeVarint(buf);
      const msgLen = decodeVarint(buf);
      const msgBytes = body.slice(buf.offset, buf.offset + msgLen);
      const msg = new TextDecoder().decode(msgBytes);
      return new RpcError(code, msg);
    } catch {
      // Fall through
    }
  }
  return new RpcError(ERR_CODE_HANDLER_ERROR, 'HTTP error');
}
