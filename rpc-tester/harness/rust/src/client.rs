//! Rust RPC test harness client.

mod generated;

use std::rc::Rc;
use std::time::Duration;

use generated::*;
use tokio::net::TcpStream;
use volex::rpc::{ERR_CODE_HANDLER_ERROR, HttpClient, PacketClient, TcpTransport};

type TestResult = Result<(), Box<dyn std::error::Error>>;
type Client<Tr> = Rc<TestServiceClient<Tr>>;

fn run_test(name: &str, result: TestResult) {
    print!("  {} ... ", name);
    match result {
        Ok(()) => println!("OK"),
        Err(e) => {
            println!("FAILED: {}", e);
            std::process::exit(1);
        }
    }
}

async fn connect_tcp(addr: &str) -> Result<Client<Rc<PacketClient<TcpTransport>>>, Box<dyn std::error::Error>> {
    let stream = TcpStream::connect(addr).await?;
    let transport = TcpTransport::new(stream);
    let packet_client = Rc::new(PacketClient::new(transport));
    let client = Rc::new(TestServiceClient::new(packet_client.clone()));

    // Spawn the packet client's run loop
    tokio::task::spawn_local({
        let packet_client = packet_client.clone();
        async move {
            if let Err(e) = packet_client.run().await {
                eprintln!("Client error: {}", e);
            }
        }
    });

    Ok(client)
}

fn connect_http(url: &str) -> Client<HttpClient> {
    Rc::new(TestServiceClient::new(HttpClient::new(url)))
}

async fn run_tests<Tr: volex::rpc::ClientTransport + 'static>(
    client: &Client<Tr>,
    skip_cancel: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("Running Rust client tests...");

    // Test echo
    run_test("echo_simple", test_echo_simple(client).await);
    run_test("echo_empty", test_echo_empty(client).await);
    run_test("echo_unicode", test_echo_unicode(client).await);

    // Test streaming
    run_test("stream_zero_items", test_stream_zero_items(client).await);
    run_test("stream_one_item", test_stream_one_item(client).await);
    run_test(
        "stream_multiple_items",
        test_stream_multiple_items(client).await,
    );

    // Test error handling
    run_test(
        "error_unary_success",
        test_error_unary_success(client).await,
    );
    run_test(
        "error_unary_failure",
        test_error_unary_failure(client).await,
    );
    run_test(
        "error_stream_after_items",
        test_error_stream_after_items(client).await,
    );

    // Test multiple simultaneous streams
    run_test(
        "multiple_simultaneous_streams",
        test_multiple_simultaneous_streams(client).await,
    );

    // Test non-message types
    run_test("non-message_add", test_non_message_add(client).await);
    run_test(
        "non-message_add_zero",
        test_non_message_add_zero(client).await,
    );
    run_test(
        "non-message_stream_strings",
        test_non_message_stream_strings(client).await,
    );

    // Test cancellation (skip for HTTP — no persistent connection for cancel signaling)
    if !skip_cancel {
        run_test(
            "cancel_unary_request",
            test_cancel_unary_request(client).await,
        );
        run_test(
            "cancel_stream_request",
            test_cancel_stream_request(client).await,
        );
    }

    println!("All tests passed!");
    Ok(())
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr = std::env::var("SERVER_ADDR").expect("SERVER_ADDR environment variable not set");
    let transport = std::env::var("TRANSPORT").unwrap_or_else(|_| "tcp".to_string());

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            match transport.as_str() {
                "tcp" => {
                    let client = connect_tcp(&addr).await?;
                    run_tests(&client, false).await
                }
                "http" => {
                    let client = connect_http(&addr);
                    run_tests(&client, true).await
                }
                other => Err(format!("unknown transport: {}", other).into()),
            }
        })
        .await
}

async fn test_echo_simple<Tr: volex::rpc::ClientTransport>(client: &Client<Tr>) -> TestResult {
    let resp = client
        .echo(EchoRequest {
            text: "hello".to_string(),
        })
        .await?;
    if resp.text != "hello" {
        return Err(format!("expected 'hello', got '{}'", resp.text).into());
    }
    Ok(())
}

async fn test_echo_empty<Tr: volex::rpc::ClientTransport>(client: &Client<Tr>) -> TestResult {
    let resp = client
        .echo(EchoRequest {
            text: "".to_string(),
        })
        .await?;
    if resp.text != "" {
        return Err(format!("expected '', got '{}'", resp.text).into());
    }
    Ok(())
}

async fn test_echo_unicode<Tr: volex::rpc::ClientTransport>(client: &Client<Tr>) -> TestResult {
    let resp = client
        .echo(EchoRequest {
            text: "Hello 世界 🌍".to_string(),
        })
        .await?;
    if resp.text != "Hello 世界 🌍" {
        return Err(format!("expected 'Hello 世界 🌍', got '{}'", resp.text).into());
    }
    Ok(())
}

async fn test_stream_zero_items<Tr: volex::rpc::ClientTransport>(
    client: &Client<Tr>,
) -> TestResult {
    let mut stream = client.subscribe(StreamRequest { count: 0 }).await?;
    let err = stream.recv().await.unwrap_err();
    if !err.is_stream_closed() {
        return Err(format!("expected stream closed error, got {:?}", err).into());
    }
    Ok(())
}

async fn test_stream_one_item<Tr: volex::rpc::ClientTransport>(
    client: &Client<Tr>,
) -> TestResult {
    let mut stream = client.subscribe(StreamRequest { count: 1 }).await?;

    let item = stream.recv().await?;
    if item.seq != 0 || item.data != "item-0" {
        return Err(format!(
            "expected {{seq: 0, data: item-0}}, got {{seq: {}, data: {}}}",
            item.seq, item.data
        )
        .into());
    }

    let err = stream.recv().await.unwrap_err();
    if !err.is_stream_closed() {
        return Err(format!("expected stream closed error, got {:?}", err).into());
    }
    Ok(())
}

async fn test_stream_multiple_items<Tr: volex::rpc::ClientTransport>(
    client: &Client<Tr>,
) -> TestResult {
    let mut stream = client.subscribe(StreamRequest { count: 5 }).await?;

    for i in 0..5u32 {
        let item = stream.recv().await?;
        let expected_data = format!("item-{}", i);
        if item.seq != i || item.data != expected_data {
            return Err(format!(
                "expected {{seq: {}, data: {}}}, got {{seq: {}, data: {}}}",
                i, expected_data, item.seq, item.data
            )
            .into());
        }
    }

    let err = stream.recv().await.unwrap_err();
    if !err.is_stream_closed() {
        return Err(format!("expected stream closed error, got {:?}", err).into());
    }
    Ok(())
}

async fn test_error_unary_success<Tr: volex::rpc::ClientTransport>(
    client: &Client<Tr>,
) -> TestResult {
    let resp = client
        .maybe_fail(FailRequest {
            should_fail: false,
            error_message: "".to_string(),
        })
        .await?;
    if !resp.success {
        return Err("expected success=true".into());
    }
    Ok(())
}

async fn test_error_unary_failure<Tr: volex::rpc::ClientTransport>(
    client: &Client<Tr>,
) -> TestResult {
    let err = client
        .maybe_fail(FailRequest {
            should_fail: true,
            error_message: "test error".to_string(),
        })
        .await
        .unwrap_err();
    if err.code != ERR_CODE_HANDLER_ERROR {
        return Err(
            format!("expected error code {}, got {}", ERR_CODE_HANDLER_ERROR, err.code).into(),
        );
    }
    if err.message != "test error" {
        return Err(format!("expected message 'test error', got '{}'", err.message).into());
    }
    Ok(())
}

async fn test_error_stream_after_items<Tr: volex::rpc::ClientTransport>(
    client: &Client<Tr>,
) -> TestResult {
    let mut stream = client.stream_then_fail(StreamRequest { count: 3 }).await?;

    for i in 0..3u32 {
        let item = stream.recv().await?;
        let expected_data = format!("item-{}", i);
        if item.seq != i || item.data != expected_data {
            return Err(format!(
                "expected {{seq: {}, data: {}}}, got {{seq: {}, data: {}}}",
                i, expected_data, item.seq, item.data
            )
            .into());
        }
    }

    let err = stream.recv().await.unwrap_err();
    if err.code != ERR_CODE_HANDLER_ERROR {
        return Err(
            format!("expected error code {}, got {}", ERR_CODE_HANDLER_ERROR, err.code).into(),
        );
    }
    Ok(())
}

async fn test_multiple_simultaneous_streams<Tr: volex::rpc::ClientTransport + 'static>(
    client: &Client<Tr>,
) -> TestResult {
    const NUM_STREAMS: usize = 5;
    let mut handles = Vec::new();

    for s in 0..NUM_STREAMS {
        let client = client.clone();
        let count = (s + 1) as u32;
        handles.push(tokio::task::spawn_local(async move {
            let mut stream = client
                .subscribe(StreamRequest { count })
                .await
                .map_err(|e| e.to_string())?;

            for i in 0..count {
                let item = stream.recv().await.map_err(|e| e.to_string())?;
                let expected_data = format!("item-{}", i);
                if item.seq != i || item.data != expected_data {
                    return Err(format!(
                        "stream {}: expected {{seq: {}, data: {}}}, got {{seq: {}, data: {}}}",
                        s, i, expected_data, item.seq, item.data
                    ));
                }
            }

            let err = stream.recv().await.unwrap_err();
            if !err.is_stream_closed() {
                return Err(format!(
                    "stream {}: expected stream closed error, got {:?}",
                    s, err
                ));
            }
            Ok::<(), String>(())
        }));
    }

    for handle in handles {
        handle
            .await?
            .map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;
    }
    Ok(())
}

async fn test_non_message_add<Tr: volex::rpc::ClientTransport>(
    client: &Client<Tr>,
) -> TestResult {
    let resp = client.add(5).await?;
    if resp != 15 {
        return Err(format!("expected 15, got {}", resp).into());
    }
    Ok(())
}

async fn test_non_message_add_zero<Tr: volex::rpc::ClientTransport>(
    client: &Client<Tr>,
) -> TestResult {
    let resp = client.add(0).await?;
    if resp != 10 {
        return Err(format!("expected 10, got {}", resp).into());
    }
    Ok(())
}

async fn test_non_message_stream_strings<Tr: volex::rpc::ClientTransport>(
    client: &Client<Tr>,
) -> TestResult {
    let mut stream = client.get_strings(3).await?;

    for i in 0..3 {
        let s = stream.recv().await?;
        let expected = format!("string-{}", i);
        if s != expected {
            return Err(format!("expected '{}', got '{}'", expected, s).into());
        }
    }

    let err = stream.recv().await.unwrap_err();
    if !err.is_stream_closed() {
        return Err(format!("expected stream closed error, got {:?}", err).into());
    }
    Ok(())
}

async fn test_cancel_unary_request<Tr: volex::rpc::ClientTransport + 'static>(
    client: &Client<Tr>,
) -> TestResult {
    let client2 = client.clone();
    let handle = tokio::task::spawn_local(async move {
        client2.slow_unary(SlowRequest { delay_ms: 5000 }).await
    });

    tokio::time::sleep(Duration::from_millis(50)).await;

    let status = client.get_status(Empty {}).await?;
    if status != "slow_unary: started" {
        return Err(format!("expected status 'slow_unary: started', got '{}'", status).into());
    }

    handle.abort();
    let _ = handle.await;

    tokio::time::sleep(Duration::from_millis(50)).await;

    let status = client.get_status(Empty {}).await?;
    if status != "slow_unary: canceled" {
        return Err(format!("expected status 'slow_unary: canceled', got '{}'", status).into());
    }
    Ok(())
}

async fn test_cancel_stream_request<Tr: volex::rpc::ClientTransport + 'static>(
    client: &Client<Tr>,
) -> TestResult {
    let mut stream = client.slow_stream(SlowRequest { delay_ms: 100 }).await?;

    for i in 0..2u32 {
        let item = stream.recv().await?;
        if item.seq != i {
            return Err(format!("expected seq {}, got {}", i, item.seq).into());
        }
    }

    let status = client.get_status(Empty {}).await?;
    if status != "slow_stream: started" {
        return Err(format!("expected status 'slow_stream: started', got '{}'", status).into());
    }

    drop(stream);

    tokio::time::sleep(Duration::from_millis(50)).await;

    let status = client.get_status(Empty {}).await?;
    if status != "slow_stream: canceled" {
        return Err(format!("expected status 'slow_stream: canceled', got '{}'", status).into());
    }
    Ok(())
}
