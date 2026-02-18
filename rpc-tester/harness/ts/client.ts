// TypeScript RPC test harness client

import * as net from 'net';
import { TcpTransport, PacketClient, HttpClient, WebSocketTransport, RpcError, ERR_CODE_HANDLER_ERROR } from 'volex/rpc';
import {
  TestServiceClient,
  EchoRequest,
  StreamRequest,
  FailRequest,
  SlowRequest,
  Empty,
} from './generated.js';

type TestResult = { ok: true } | { ok: false; error: string };

function ok(): TestResult {
  return { ok: true };
}

function fail(msg: string): TestResult {
  return { ok: false, error: msg };
}

async function runTest(name: string, testFn: () => Promise<TestResult>): Promise<boolean> {
  process.stdout.write(`  ${name} ... `);
  try {
    const result = await testFn();
    if (result.ok) {
      console.log('OK');
      return true;
    } else {
      console.log(`FAILED: ${result.error}`);
      return false;
    }
  } catch (e) {
    console.log(`FAILED: ${e}`);
    return false;
  }
}

// Test functions

async function testEchoSimple(client: TestServiceClient): Promise<TestResult> {
  const resp = await client.echo({ text: 'hello' });
  if (resp.text !== 'hello') {
    return fail(`expected 'hello', got '${resp.text}'`);
  }
  return ok();
}

async function testEchoEmpty(client: TestServiceClient): Promise<TestResult> {
  const resp = await client.echo({ text: '' });
  if (resp.text !== '') {
    return fail(`expected '', got '${resp.text}'`);
  }
  return ok();
}

async function testEchoUnicode(client: TestServiceClient): Promise<TestResult> {
  const resp = await client.echo({ text: 'Hello 世界 🌍' });
  if (resp.text !== 'Hello 世界 🌍') {
    return fail(`expected 'Hello 世界 🌍', got '${resp.text}'`);
  }
  return ok();
}

async function testStreamZeroItems(client: TestServiceClient): Promise<TestResult> {
  const stream = await client.subscribe({ count: 0 });
  try {
    await stream.recv();
    return fail('expected stream closed error');
  } catch (e) {
    if (e instanceof RpcError && e.isStreamClosed()) {
      return ok();
    }
    return fail(`expected stream closed error, got ${e}`);
  }
}

async function testStreamOneItem(client: TestServiceClient): Promise<TestResult> {
  const stream = await client.subscribe({ count: 1 });

  const item = await stream.recv();
  if (item.seq !== 0 || item.data !== 'item-0') {
    return fail(`expected {seq: 0, data: item-0}, got {seq: ${item.seq}, data: ${item.data}}`);
  }

  try {
    await stream.recv();
    return fail('expected stream closed error');
  } catch (e) {
    if (e instanceof RpcError && e.isStreamClosed()) {
      return ok();
    }
    return fail(`expected stream closed error, got ${e}`);
  }
}

async function testStreamMultipleItems(client: TestServiceClient): Promise<TestResult> {
  const stream = await client.subscribe({ count: 5 });

  for (let i = 0; i < 5; i++) {
    const item = await stream.recv();
    const expectedData = `item-${i}`;
    if (item.seq !== i || item.data !== expectedData) {
      return fail(`expected {seq: ${i}, data: ${expectedData}}, got {seq: ${item.seq}, data: ${item.data}}`);
    }
  }

  try {
    await stream.recv();
    return fail('expected stream closed error');
  } catch (e) {
    if (e instanceof RpcError && e.isStreamClosed()) {
      return ok();
    }
    return fail(`expected stream closed error, got ${e}`);
  }
}

async function testErrorUnarySuccess(client: TestServiceClient): Promise<TestResult> {
  const resp = await client.maybe_fail({
    should_fail: false,
    error_message: '',
  });
  if (!resp.success) {
    return fail('expected success=true');
  }
  return ok();
}

async function testErrorUnaryFailure(client: TestServiceClient): Promise<TestResult> {
  try {
    await client.maybe_fail({
      should_fail: true,
      error_message: 'test error',
    });
    return fail('expected error');
  } catch (e) {
    if (!(e instanceof RpcError)) {
      return fail(`expected RpcError, got ${e}`);
    }
    if (e.code !== ERR_CODE_HANDLER_ERROR) {
      return fail(`expected error code ${ERR_CODE_HANDLER_ERROR}, got ${e.code}`);
    }
    if (e.message !== 'test error') {
      return fail(`expected message 'test error', got '${e.message}'`);
    }
    return ok();
  }
}

async function testErrorStreamAfterItems(client: TestServiceClient): Promise<TestResult> {
  const stream = await client.stream_then_fail({ count: 3 });

  // Should receive 3 items first
  for (let i = 0; i < 3; i++) {
    const item = await stream.recv();
    const expectedData = `item-${i}`;
    if (item.seq !== i || item.data !== expectedData) {
      return fail(`expected {seq: ${i}, data: ${expectedData}}, got {seq: ${item.seq}, data: ${item.data}}`);
    }
  }

  // Then should get an error
  try {
    await stream.recv();
    return fail('expected error after stream items');
  } catch (e) {
    if (!(e instanceof RpcError)) {
      return fail(`expected RpcError, got ${e}`);
    }
    if (e.code !== ERR_CODE_HANDLER_ERROR) {
      return fail(`expected error code ${ERR_CODE_HANDLER_ERROR}, got ${e.code}`);
    }
    return ok();
  }
}

async function testMultipleSimultaneousStreams(client: TestServiceClient): Promise<TestResult> {
  const NUM_STREAMS = 5;
  const promises: Promise<TestResult>[] = [];

  for (let s = 0; s < NUM_STREAMS; s++) {
    const count = s + 1;
    const streamIdx = s;
    promises.push(
      (async (): Promise<TestResult> => {
        const stream = await client.subscribe({ count });

        for (let i = 0; i < count; i++) {
          const item = await stream.recv();
          const expectedData = `item-${i}`;
          if (item.seq !== i || item.data !== expectedData) {
            return fail(
              `stream ${streamIdx}: expected {seq: ${i}, data: ${expectedData}}, got {seq: ${item.seq}, data: ${item.data}}`
            );
          }
        }

        try {
          await stream.recv();
          return fail(`stream ${streamIdx}: expected stream closed error`);
        } catch (e) {
          if (e instanceof RpcError && e.isStreamClosed()) {
            return ok();
          }
          return fail(`stream ${streamIdx}: expected stream closed error, got ${e}`);
        }
      })()
    );
  }

  const results = await Promise.all(promises);
  for (const result of results) {
    if (!result.ok) {
      return result;
    }
  }
  return ok();
}

async function testNonMessageAdd(client: TestServiceClient): Promise<TestResult> {
  const resp = await client.add(5);
  if (resp !== 15) {
    return fail(`expected 15, got ${resp}`);
  }
  return ok();
}

async function testNonMessageAddZero(client: TestServiceClient): Promise<TestResult> {
  const resp = await client.add(0);
  if (resp !== 10) {
    return fail(`expected 10, got ${resp}`);
  }
  return ok();
}

async function testNonMessageStreamStrings(client: TestServiceClient): Promise<TestResult> {
  const stream = await client.get_strings(3);

  for (let i = 0; i < 3; i++) {
    const s = await stream.recv();
    const expected = `string-${i}`;
    if (s !== expected) {
      return fail(`expected '${expected}', got '${s}'`);
    }
  }

  try {
    await stream.recv();
    return fail('expected stream closed error');
  } catch (e) {
    if (e instanceof RpcError && e.isStreamClosed()) {
      return ok();
    }
    return fail(`expected stream closed error, got ${e}`);
  }
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

async function testStreamNotBuffered(client: TestServiceClient): Promise<TestResult> {
  // Start a slow stream (items every 200ms). The stream is infinite, so if the
  // client buffers the entire response before delivering items, it will hang forever.
  const ac = new AbortController();
  const stream = await client.slow_stream({ delay_ms: 200 }, { signal: ac.signal });

  // Receive the first item with a timeout. A properly streaming client delivers
  // this in ~200ms. A buffering client never delivers it (infinite stream).
  const first = await Promise.race([
    stream.recv().then((item) => ({ tag: 'ok' as const, item })),
    sleep(2000).then(() => ({ tag: 'timeout' as const })),
  ]);
  if (first.tag === 'timeout') {
    return fail('timed out waiting for first stream item — client is likely buffering the response');
  }
  if (first.item.seq !== 0) {
    return fail(`expected seq 0, got ${first.item.seq}`);
  }

  // Receive a second item to double-check.
  const second = await Promise.race([
    stream.recv().then((item) => ({ tag: 'ok' as const, item })),
    sleep(2000).then(() => ({ tag: 'timeout' as const })),
  ]);
  if (second.tag === 'timeout') {
    return fail('timed out waiting for second stream item');
  }
  if (second.item.seq !== 1) {
    return fail(`expected seq 1, got ${second.item.seq}`);
  }

  // Abort the stream and wait for the server to process the cancellation,
  // so we don't leave stale last_status that interferes with later tests.
  ac.abort();
  await sleep(50);

  return ok();
}

async function testCancelUnaryRequest(client: TestServiceClient): Promise<TestResult> {
  const ac = new AbortController();

  // Start the slow request — we'll cancel it before it completes
  const slowPromise = client.slow_unary({ delay_ms: 5000 }, { signal: ac.signal });

  // Wait a bit for the server to start processing
  await sleep(50);

  // Verify server has started but not yet canceled
  const status1 = await client.get_status({});
  if (status1 !== 'slow_unary: started') {
    return fail(`expected status 'slow_unary: started', got '${status1}'`);
  }

  // Cancel the request
  ac.abort();

  // The promise should reject
  try {
    await slowPromise;
    return fail('expected error after cancellation');
  } catch (e) {
    // Expected — aborted
  }

  // Give the server a moment to process the cancellation
  await sleep(50);

  // Verify server saw the cancellation
  const status2 = await client.get_status({});
  if (status2 !== 'slow_unary: canceled') {
    return fail(`expected status 'slow_unary: canceled', got '${status2}'`);
  }

  return ok();
}

async function testCancelStreamRequest(client: TestServiceClient): Promise<TestResult> {
  const ac = new AbortController();

  // Start a slow stream (items every 100ms)
  const stream = await client.slow_stream({ delay_ms: 100 }, { signal: ac.signal });

  // Receive a couple items
  for (let i = 0; i < 2; i++) {
    const item = await stream.recv();
    if (item.seq !== i) {
      return fail(`expected seq ${i}, got ${item.seq}`);
    }
  }

  // Verify server has started but not yet canceled
  const status1 = await client.get_status({});
  if (status1 !== 'slow_stream: started') {
    return fail(`expected status 'slow_stream: started', got '${status1}'`);
  }

  // Abort the stream
  ac.abort();

  // After aborting, recv() should throw an error
  try {
    await stream.recv();
    return fail('expected error after abort');
  } catch {
    // Expected — aborted
  }

  // Give the server a moment to process the cancellation
  await sleep(50);

  // Verify server saw the cancellation
  const status2 = await client.get_status({});
  if (status2 !== 'slow_stream: canceled') {
    return fail(`expected status 'slow_stream: canceled', got '${status2}'`);
  }

  return ok();
}

async function testCancelWhileRecvWaiting(client: TestServiceClient): Promise<TestResult> {
  const ac = new AbortController();

  // Start a slow stream (items every 500ms - slow enough that we can cancel while waiting)
  const stream = await client.slow_stream({ delay_ms: 500 }, { signal: ac.signal });

  // Receive the first item
  const item = await stream.recv();
  if (item.seq !== 0) {
    return fail(`expected seq 0, got ${item.seq}`);
  }

  // Start a recv() that will block waiting for the next item (which won't come for 500ms)
  const recvPromise = stream.recv();

  // Wait a bit to ensure recv() is actually waiting
  await sleep(50);

  // Abort the stream while recv() is waiting
  ac.abort();

  // The recv() should immediately throw (not wait for the 500ms item delay)
  const startTime = Date.now();
  try {
    await recvPromise;
    return fail('expected error from waiting recv() after abort');
  } catch {
    const elapsed = Date.now() - startTime;
    // Should resolve almost immediately
    if (elapsed > 100) {
      return fail(`recv() took too long to cancel: ${elapsed}ms`);
    }
  }

  return ok();
}

async function connectTcp(addr: string): Promise<{ client: TestServiceClient; close: () => void }> {
  const [host, portStr] = addr.split(':');
  const port = parseInt(portStr, 10);

  const socket = net.createConnection({ host, port });
  await new Promise<void>((resolve, reject) => {
    socket.on('connect', resolve);
    socket.on('error', reject);
  });

  const transport = new TcpTransport(socket);
  const packetClient = new PacketClient(transport);
  const client = new TestServiceClient(packetClient);

  // Spawn the packet client's run loop
  packetClient.run().catch((e: unknown) => {
    // Connection closed errors are expected at the end
    if (!(e instanceof Error && e.message.includes('closed'))) {
      console.error('Client error:', e);
    }
  });

  return { client, close: () => transport.close() };
}

async function runAllTests(client: TestServiceClient): Promise<boolean> {
  let allPassed = true;

  // Test echo
  allPassed = (await runTest('echo_simple', () => testEchoSimple(client))) && allPassed;
  allPassed = (await runTest('echo_empty', () => testEchoEmpty(client))) && allPassed;
  allPassed = (await runTest('echo_unicode', () => testEchoUnicode(client))) && allPassed;

  // Test streaming
  allPassed = (await runTest('stream_zero_items', () => testStreamZeroItems(client))) && allPassed;
  allPassed = (await runTest('stream_one_item', () => testStreamOneItem(client))) && allPassed;
  allPassed = (await runTest('stream_multiple_items', () => testStreamMultipleItems(client))) && allPassed;

  // Test error handling
  allPassed = (await runTest('error_unary_success', () => testErrorUnarySuccess(client))) && allPassed;
  allPassed = (await runTest('error_unary_failure', () => testErrorUnaryFailure(client))) && allPassed;
  allPassed = (await runTest('error_stream_after_items', () => testErrorStreamAfterItems(client))) && allPassed;

  // Test multiple simultaneous streams
  allPassed = (await runTest('multiple_simultaneous_streams', () => testMultipleSimultaneousStreams(client))) && allPassed;

  // Test non-message types
  allPassed = (await runTest('non-message_add', () => testNonMessageAdd(client))) && allPassed;
  allPassed = (await runTest('non-message_add_zero', () => testNonMessageAddZero(client))) && allPassed;
  allPassed = (await runTest('non-message_stream_strings', () => testNonMessageStreamStrings(client))) && allPassed;

  // Test that streaming responses are not buffered
  allPassed = (await runTest('stream_not_buffered', () => testStreamNotBuffered(client))) && allPassed;

  // Test cancellation
  allPassed = (await runTest('cancel_unary_request', () => testCancelUnaryRequest(client))) && allPassed;
  allPassed = (await runTest('cancel_stream_request', () => testCancelStreamRequest(client))) && allPassed;
  allPassed = (await runTest('cancel_while_recv_waiting', () => testCancelWhileRecvWaiting(client))) && allPassed;

  return allPassed;
}

async function main() {
  const addr = process.env.SERVER_ADDR;
  if (!addr) {
    console.error('SERVER_ADDR environment variable not set');
    process.exit(1);
  }

  const transportType = process.env.TRANSPORT || 'tcp';

  let client: TestServiceClient;
  let close: () => void;

  switch (transportType) {
    case 'tcp': {
      const conn = await connectTcp(addr);
      client = conn.client;
      close = conn.close;
      break;
    }
    case 'http': {
      client = new TestServiceClient(new HttpClient(addr));
      close = () => {};
      break;
    }
    case 'ws': {
      const ws = new WebSocket(addr);
      await new Promise<void>((resolve, reject) => {
        ws.addEventListener('open', () => resolve());
        ws.addEventListener('error', (e) => reject(e));
      });
      const transport = new WebSocketTransport(ws);
      const packetClient = new PacketClient(transport);
      client = new TestServiceClient(packetClient);
      packetClient.run().catch((e: unknown) => {
        if (!(e instanceof Error && e.message.includes('closed'))) {
          console.error('Client error:', e);
        }
      });
      close = () => transport.close();
      break;
    }
    default:
      throw new Error(`unknown transport: ${transportType}`);
  }

  console.log('Running TypeScript client tests...');

  const allPassed = await runAllTests(client);

  // Close the connection
  close();

  if (allPassed) {
    console.log('All tests passed!');
    process.exit(0);
  } else {
    process.exit(1);
  }
}

main().catch((e) => {
  console.error('Fatal error:', e);
  process.exit(1);
});
