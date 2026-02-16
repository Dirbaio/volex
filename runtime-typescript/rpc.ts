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
// High-level Transport Interfaces
// ============================================================================

export interface ClientTransport {
  callUnary(methodIndex: number, payload: Uint8Array): Promise<Uint8Array>;
  callStream(methodIndex: number, payload: Uint8Array): Promise<StreamReceiver>;
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

  async callUnary(methodIndex: number, payload: Uint8Array): Promise<Uint8Array> {
    if (this.recvError) {
      throw new RpcError(0, this.recvError.message);
    }

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

    // Wait for response
    return responsePromise;
  }

  async callStream(methodIndex: number, payload: Uint8Array): Promise<StreamReceiver> {
    if (this.recvError) {
      throw new RpcError(0, this.recvError.message);
    }

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

    return new PacketStreamReceiver(
      requestId,
      pendingStream,
      this.pending,
      this.transport,
    );
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
  cancel(): Promise<void>;
}

class PacketStreamReceiver implements StreamReceiver {
  private requestId: number;
  private pending: PendingStream;
  private allPending: Map<number, PendingRequest>;
  private transport: PacketTransport;
  private cancelled = false;

  constructor(
    requestId: number,
    pending: PendingStream,
    allPending: Map<number, PendingRequest>,
    transport: PacketTransport,
  ) {
    this.requestId = requestId;
    this.pending = pending;
    this.allPending = allPending;
    this.transport = transport;
  }

  async recv(): Promise<Uint8Array> {
    if (this.cancelled) {
      throw RpcError.streamCancelled();
    }

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

  async cancel(): Promise<void> {
    if (this.cancelled) {
      return;
    }
    this.cancelled = true;

    this.allPending.delete(this.requestId);

    // Wake up any waiting recv() calls with a cancellation event
    const cancelEvent: StreamEvent = { type: 'error', error: RpcError.streamCancelled() };
    for (const waiter of this.pending.waiters) {
      waiter(cancelEvent);
    }
    this.pending.waiters = [];

    // Send cancel message
    const buf = new WriteBuf();
    buf.pushByte(RPC_TYPE_CANCEL);
    encodeVarint(this.requestId, buf);
    const packet = buf.toUint8Array();
    try {
      await this.transport.send(packet);
    } catch {
      // Best effort
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

  async callUnary(methodIndex: number, payload: Uint8Array): Promise<Uint8Array> {
    const body = this.buildRequestBody(methodIndex, payload);
    const resp = await this.doRequest(body);

    if (resp.statusCode !== 200) {
      throw parseHttpError(resp.contentType, resp.body);
    }

    return resp.body;
  }

  async callStream(methodIndex: number, payload: Uint8Array): Promise<StreamReceiver> {
    const body = this.buildRequestBody(methodIndex, payload);
    const resp = await this.doRequest(body);

    if (resp.statusCode !== 200) {
      throw parseHttpError(resp.contentType, resp.body);
    }

    // Parse stream messages from body
    const queue: StreamEvent[] = [];
    const waiters: Array<(event: StreamEvent) => void> = [];

    let sbuf = new Buf(resp.body, 0);
    while (sbuf.offset < sbuf.data.length) {
      const length = decodeVarint(sbuf);
      if (sbuf.offset + length > sbuf.data.length) {
        queue.push({ type: 'error', error: new RpcError(0, 'truncated stream message') });
        break;
      }
      const msg = sbuf.data.slice(sbuf.offset, sbuf.offset + length);
      sbuf.offset += length;

      if (msg.length === 0) {
        queue.push({ type: 'error', error: new RpcError(0, 'empty stream message') });
        break;
      }

      const msgType = msg[0];
      const msgPayload = msg.slice(1);

      switch (msgType) {
        case RPC_TYPE_STREAM_ITEM:
          queue.push({ type: 'item', data: msgPayload });
          break;
        case RPC_TYPE_STREAM_END:
          queue.push({ type: 'end' });
          break;
        case RPC_TYPE_ERROR: {
          const ebuf = new Buf(msgPayload, 0);
          const code = decodeVarint(ebuf);
          const msgLen = decodeVarint(ebuf);
          const msgBytes = msgPayload.slice(ebuf.offset, ebuf.offset + msgLen);
          const errMsg = new TextDecoder().decode(msgBytes);
          queue.push({ type: 'error', error: new RpcError(code, errMsg) });
          break;
        }
        default:
          queue.push({ type: 'error', error: new RpcError(0, `unknown stream message type: 0x${msgType.toString(16)}`) });
          break;
      }

      // Stop after end or error
      const last = queue[queue.length - 1];
      if (last.type === 'end' || last.type === 'error') {
        break;
      }
    }

    // If no end marker was found, add one
    if (queue.length === 0 || (queue[queue.length - 1].type !== 'end' && queue[queue.length - 1].type !== 'error')) {
      queue.push({ type: 'end' });
    }

    const pendingStream: PendingStream = { type: 'stream', queue, waiters };

    return new HttpStreamReceiver(pendingStream);
  }

  private buildRequestBody(methodIndex: number, payload: Uint8Array): Uint8Array {
    const wbuf = new WriteBuf();
    encodeVarint(methodIndex, wbuf);
    wbuf.push(payload);
    return wbuf.toUint8Array();
  }

  private async doRequest(body: Uint8Array): Promise<{ statusCode: number; contentType: string; body: Uint8Array }> {
    const resp = await fetch(this.url, {
      method: 'POST',
      headers: {
        'Content-Type': CONTENT_TYPE_RPC,
      },
      body: body as Uint8Array<ArrayBuffer>,
    });

    const respBody = new Uint8Array(await resp.arrayBuffer());
    return {
      statusCode: resp.status,
      contentType: resp.headers.get('content-type') || '',
      body: respBody,
    };
  }
}

class HttpStreamReceiver {
  private pending: PendingStream;
  private cancelled = false;

  constructor(pending: PendingStream) {
    this.pending = pending;
  }

  async recv(): Promise<Uint8Array> {
    if (this.cancelled) {
      throw RpcError.streamCancelled();
    }

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

  async cancel(): Promise<void> {
    this.cancelled = true;
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
